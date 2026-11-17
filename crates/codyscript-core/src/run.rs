//! Running a compiled program.
//!
//! The interpreter is a machine with explicit stacks — tasks to do, values
//! produced, block results, scopes — driven by one loop. Nothing recurses, so
//! the stack a run uses doesn't grow with the program's depth, nor with how
//! deeply its functions call each other: neither can overflow it, natively or
//! under WebAssembly's small stack. The loop's only `.await` is an executor
//! call.
//!
//! Every step is counted at the moment the recursive definition (the
//! TypeScript engine) counts it, and in the same order: an instruction or
//! input is a step when it starts, before its parts.
//!
//! Work that would be pushed and popped straight away is done at once
//! instead: a list runs its first instruction, and an instruction resolves
//! its input. Only those two calls are direct (resolving an input never runs
//! instructions itself; a block's are pushed as a task), so the native stack
//! stays the same depth however the program nests.

use crate::engine::{Limits, RunOptions, RunResult, Variables};
use crate::error::{Cause, Details, Error, ErrorCode};
use crate::executor::{ExecutorCall, Signal};
use crate::expression::{self, Fault, Stop};
use crate::program::{
    Call, Expression, External, Input, InputKind, Instruction, InstructionKind, Program,
};
use crate::value::{Map, Value};

/// Runs `program`'s body to completion.
pub(crate) async fn run(
    program: &Program,
    limits: Limits,
    options: RunOptions,
) -> Result<RunResult, Error> {
    let mut machine = Machine {
        functions: &program.functions,
        meter: Meter {
            steps: 0,
            max_steps: limits.max_steps,
            signal: options.signal,
        },
        max_depth: limits.max_depth,
        scopes: vec![options.variables],
        base: 0,
        calls: 0,
        tasks: Vec::new(),
        values: Vec::new(),
        results: Vec::new(),
    };
    // The body runs in the root scope, so its assignments are the result's
    // variables.
    machine.enter(&program.body);
    machine.drive().await?;
    Ok(RunResult {
        value: machine.values.pop().unwrap_or_default(),
        variables: machine.scopes.swap_remove(0),
        steps: machine.meter.steps,
    })
}

/// What's left to do. Tasks that need a value find it on the value stack.
enum Task<'p> {
    /// Produce this input's value.
    Resolve(&'p Input),
    /// Run these instructions in order, in the current block.
    List(&'p [Instruction]),
    /// Call the external function with the arguments on the value stack.
    Call(&'p External, &'p Input),
    /// Run the function with the arguments on the value stack as its
    /// variables.
    Invoke(&'p Call, &'p Input),
    /// Assign the value to this variable in the current scope.
    Assign(&'p str),
    /// End the current block with the value.
    Return,
    /// Run `then` if the value is truthy, else `otherwise`.
    Branch {
        then: &'p [Instruction],
        otherwise: Option<&'p [Instruction]>,
    },
    /// If the value (the loop's test) is truthy, run `body` and test again.
    Loop {
        test: &'p Input,
        body: &'p [Instruction],
    },
    /// The current block is over: its result becomes a value, and its
    /// scope, if it has its own, goes.
    EndBlock { scoped: bool },
    /// The current function is over: its value stays, its scopes go, and
    /// the caller's, from `base` up, are visible again.
    EndCall { base: usize },
}

/// One run's state.
struct Machine<'p> {
    /// The program's function bodies, by index.
    functions: &'p [Input],
    meter: Meter,
    /// How deeply calls may nest.
    max_depth: usize,
    /// Variables: a block pushes its own scope, reads fall through to the
    /// enclosing ones, and `set` writes the innermost.
    scopes: Vec<Variables>,
    /// Where the running function's scopes start: reads don't fall through
    /// below it, so a function sees only its own variables.
    base: usize,
    /// How many calls are running.
    calls: usize,
    tasks: Vec<Task<'p>>,
    values: Vec<Value>,
    /// What each running block will return (`null` until a `ret`).
    results: Vec<Value>,
}

impl<'p> Machine<'p> {
    async fn drive(&mut self) -> Result<(), Error> {
        while let Some(task) = self.tasks.pop() {
            if let Task::Call(operation, node) = task {
                let value = self.call(operation, node).await?;
                self.values.push(value);
            } else {
                self.perform(task)?;
            }
        }
        Ok(())
    }

    /// Does one task that needs no waiting.
    fn perform(&mut self, task: Task<'p>) -> Result<(), Error> {
        match task {
            Task::Resolve(input) => self.resolve(input)?,
            Task::List(list) => self.list(list)?,
            Task::Assign(name) => {
                let value = self.pop();
                // Reassigning, the usual case in a loop, needs no new name.
                let scope = self.scope();
                match scope.get_mut(name) {
                    Some(slot) => *slot = value,
                    None => {
                        scope.insert(name.to_owned(), value);
                    }
                }
            }
            Task::Return => self.ret(),
            Task::Branch { then, otherwise } => {
                if truthy(&self.pop()) {
                    self.list(then)?;
                } else if let Some(otherwise) = otherwise {
                    self.list(otherwise)?;
                }
            }
            Task::Loop { test, body } => {
                if truthy(&self.pop()) {
                    self.tasks.push(Task::Loop { test, body });
                    self.tasks.push(Task::Resolve(test));
                    self.list(body)?;
                }
            }
            Task::EndBlock { scoped } => {
                if scoped {
                    self.scopes.pop();
                }
                let result = self.results.pop().unwrap_or_default();
                self.values.push(result);
            }
            Task::Invoke(call, node) => self.invoke(call, node)?,
            Task::EndCall { base } => {
                self.scopes.truncate(self.base);
                self.base = base;
                self.calls -= 1;
            }
            Task::Call(..) => unreachable!("calls are awaited by drive()"),
        }
        Ok(())
    }

    fn resolve(&mut self, input: &'p Input) -> Result<(), Error> {
        self.step(&input.path, input.tag.as_deref())?;
        let value = match &input.kind {
            InputKind::Const(value) => value.clone(),
            InputKind::Ref(name) => match self.get(name) {
                Some(value) => value.clone(),
                None => {
                    let message = format!("Variable \"{name}\" is not defined");
                    let code = ErrorCode::UndefinedVariable;
                    return Err(failure(
                        code,
                        message,
                        &input.path,
                        input.tag.as_deref(),
                        None,
                    ));
                }
            },
            InputKind::X(expression) => self.expression(expression, input.tag.as_deref())?,
            InputKind::External(external) => {
                // Arguments resolve one at a time, in order: the first
                // pushed last, so it runs first.
                self.tasks.push(Task::Call(external, input));
                for (_, argument) in external.args.iter().rev() {
                    self.tasks.push(Task::Resolve(argument));
                }
                return Ok(());
            }
            InputKind::Do(list) => {
                self.enter_block(list, true);
                return Ok(());
            }
            InputKind::Call(call) => {
                // As for an external function: arguments first, in order.
                self.tasks.push(Task::Invoke(call, input));
                for (_, argument) in call.args.iter().rev() {
                    self.tasks.push(Task::Resolve(argument));
                }
                return Ok(());
            }
        };
        self.values.push(value);
        Ok(())
    }

    /// Runs `list`'s first instruction now, and the rest after it.
    fn list(&mut self, list: &'p [Instruction]) -> Result<(), Error> {
        let Some((first, rest)) = list.split_first() else {
            return Ok(());
        };
        if !rest.is_empty() {
            self.tasks.push(Task::List(rest));
        }
        self.execute(first)
    }

    /// Runs an instruction: resolves its input now, and pushes what to do
    /// with the value.
    fn execute(&mut self, ins: &'p Instruction) -> Result<(), Error> {
        self.step(&ins.path, ins.tag.as_deref())?;
        let input = match &ins.kind {
            InstructionKind::Set { name, to } => {
                self.tasks.push(Task::Assign(name));
                to
            }
            InstructionKind::Ret(input) => {
                self.tasks.push(Task::Return);
                input
            }
            InstructionKind::If {
                test,
                then,
                otherwise,
            } => {
                self.tasks.push(Task::Branch {
                    then,
                    otherwise: otherwise.as_deref(),
                });
                test
            }
            InstructionKind::For { test, body } => {
                self.tasks.push(Task::Loop { test, body });
                test
            }
        };
        self.resolve(input)
    }

    /// Starts a function's body — or the program's — in the current scope.
    /// A body that is a block runs its instructions there directly; like any
    /// program's top-level block, it isn't a step of its own.
    fn enter(&mut self, body: &'p Input) {
        match &body.kind {
            InputKind::Do(list) => self.enter_block(list, false),
            _ => self.tasks.push(Task::Resolve(body)),
        }
    }

    /// Calls a function: its arguments, on the value stack, become the
    /// variables of a fresh scope that the caller's can't be read through,
    /// and its body runs there until `EndCall`.
    fn invoke(&mut self, call: &'p Call, node: &'p Input) -> Result<(), Error> {
        if self.calls >= self.max_depth {
            let message = format!("Calls nest deeper than {} levels", self.max_depth);
            let tag = node.tag.as_deref();
            return Err(failure(
                ErrorCode::DepthLimit,
                message,
                &node.path,
                tag,
                None,
            ));
        }
        let variables = self.arguments(&call.args);
        self.calls += 1;
        self.tasks.push(Task::EndCall { base: self.base });
        self.base = self.scopes.len();
        self.scopes.push(variables);
        let functions = self.functions;
        self.enter(&functions[call.function]);
        Ok(())
    }

    /// Starts a block: its own scope (unless it's the top-level one), a
    /// `null` result, and its instructions.
    fn enter_block(&mut self, list: &'p [Instruction], scoped: bool) {
        if scoped {
            self.scopes.push(Variables::new());
        }
        self.results.push(Value::Null);
        self.tasks.push(Task::EndBlock { scoped });
        self.tasks.push(Task::List(list));
    }

    /// `ret`: the current block's result is the value, and whatever the
    /// block had left to do — the rest of its instructions, a loop's next
    /// test — is dropped, up to the block's end.
    fn ret(&mut self) {
        let value = self.pop();
        if let Some(result) = self.results.last_mut() {
            *result = value;
        }
        while self
            .tasks
            .pop_if(|task| !matches!(task, Task::EndBlock { .. }))
            .is_some()
        {}
    }

    /// The arguments on the value stack, by name, taken off it.
    fn arguments(&mut self, args: &[(String, Input)]) -> Map {
        let at = self.values.len().saturating_sub(args.len());
        let mut map = Map::with_capacity(args.len());
        let names = args.iter().map(|(name, _)| name.clone());
        map.extend(names.zip(self.values.drain(at..)));
        map
    }

    async fn call(&mut self, external: &External, node: &Input) -> Result<Value, Error> {
        let tag = node.tag.as_deref();
        let call = ExecutorCall {
            op: external.op.clone(),
            arg: external.arg.clone(),
            input: self.arguments(&external.args),
            signal: self.meter.signal.clone(),
        };
        let name = || format!("{}.{}", external.exe, external.op);
        external.executor.call(call).await.map_err(|error| {
            match error.downcast::<Error>() {
                // An executor's own engine error passes through unchanged.
                Ok(own) => *own,
                Err(cause) => {
                    let message = format!("Function \"{}\" failed: {cause}", name());
                    let cause = Some(Cause::from(cause));
                    failure(ErrorCode::FunctionFailed, message, &node.path, tag, cause)
                }
            }
        })
    }

    fn expression(&mut self, expression: &Expression, tag: Option<&str>) -> Result<Value, Error> {
        let mut cx = ExpressionContext {
            scopes: &self.scopes[self.base..],
            meter: &mut self.meter,
        };
        expression::evaluate(&expression.ast, &mut cx).map_err(|fault| {
            let (code, message) = match fault {
                Fault::Stop(stop) => return self.meter.error(stop, &expression.path, tag),
                Fault::Undefined(name) => (
                    ErrorCode::UndefinedVariable,
                    format!("Variable \"{name}\" is not defined"),
                ),
                Fault::Type(message) => (ErrorCode::TypeError, message),
            };
            let message = format!("{message} in \"{}\"", expression.source);
            failure(code, message, &expression.path, tag, None)
        })
    }

    fn step(&mut self, path: &str, tag: Option<&str>) -> Result<(), Error> {
        self.meter
            .step()
            .map_err(|stop| self.meter.error(stop, path, tag))
    }

    /// A variable the running function can see.
    fn get(&self, name: &str) -> Option<&Value> {
        lookup(&self.scopes[self.base..], name)
    }

    fn scope(&mut self) -> &mut Variables {
        // The root scope is never popped, nor a function's while it runs.
        let last = self.scopes.len() - 1;
        &mut self.scopes[last]
    }

    /// The value a task needs. The machine always pushes one first.
    fn pop(&mut self) -> Value {
        self.values.pop().unwrap_or_default()
    }
}

/// A variable, from the innermost scope that has it.
fn lookup<'v>(scopes: &'v [Variables], name: &str) -> Option<&'v Value> {
    scopes.iter().rev().find_map(|scope| scope.get(name))
}

/// What an expression reads and counts while it's evaluated.
struct ExpressionContext<'v, 'm> {
    scopes: &'v [Variables],
    meter: &'m mut Meter,
}

impl<'v> expression::Context<'v> for ExpressionContext<'v, '_> {
    // `null` is a value; only a missing variable is undefined.
    fn get(&self, name: &str) -> Option<&'v Value> {
        lookup(self.scopes, name)
    }

    #[inline]
    fn step(&mut self) -> Result<(), Stop> {
        self.meter.step()
    }
}

/// A run's steps: how many it has taken, how many it may, and the signal
/// that stops it.
struct Meter {
    steps: u64,
    max_steps: u64,
    signal: Option<Signal>,
}

impl Meter {
    /// Counts one step, unless the run is aborted or out of steps.
    #[inline]
    fn step(&mut self) -> Result<(), Stop> {
        if self.signal.as_ref().is_some_and(Signal::is_aborted) {
            return Err(Stop::Aborted);
        }
        if self.steps >= self.max_steps {
            return Err(Stop::StepLimit);
        }
        self.steps += 1;
        Ok(())
    }

    /// The error for a step refused at `path`.
    #[cold]
    fn error(&self, stop: Stop, path: &str, tag: Option<&str>) -> Error {
        match stop {
            Stop::Aborted => {
                let reason = self.signal.as_ref().and_then(Signal::reason);
                failure(ErrorCode::Aborted, "Run aborted", path, tag, reason)
            }
            Stop::StepLimit => {
                let message = format!("Program exceeded {} steps", self.max_steps);
                failure(ErrorCode::StepLimit, message, path, tag, None)
            }
        }
    }
}

fn failure(
    code: ErrorCode,
    message: impl Into<String>,
    path: &str,
    tag: Option<&str>,
    cause: Option<Cause>,
) -> Error {
    Error::with_details(
        code,
        message,
        Details {
            path: Some(path.to_owned()),
            tag: tag.map(str::to_owned),
            issues: None,
            cause,
        },
    )
}

/// JavaScript truthiness: `false`, `0`, `NaN`, `""` and `null` are false;
/// everything else, including `[]` and `{}`, is true.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            let n = n.as_f64();
            n != 0.0 && !n.is_nan()
        }
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}
