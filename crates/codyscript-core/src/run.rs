//! Running a compiled program.
//!
//! The interpreter is a machine with explicit stacks — tasks to do, values
//! produced, block results, scopes — driven by one loop. Nothing recurses, so
//! the stack a run uses doesn't grow with the program's depth: a deeply
//! nested program can't overflow it, natively or under WebAssembly's small
//! stack. The loop's only `.await` is an executor call.
//!
//! Every step is counted at the moment the recursive definition (the
//! TypeScript engine) counts it, and in the same order: an instruction or
//! input is a step when it starts, before its parts.

use crate::engine::{Executors, RunOptions, RunResult, Variables};
use crate::error::{Cause, Details, Error, ErrorCode};
use crate::executor::{ExecutorCall, Signal};
use crate::expression::{self, Fault};
use crate::program::{Expression, Input, InputKind, Instruction, InstructionKind, Operation};
use crate::value::{Map, Value};

/// Runs `root` to completion.
pub(crate) async fn run(
    root: &Input,
    executors: &Executors,
    max_steps: u64,
    options: RunOptions,
) -> Result<RunResult, Error> {
    let mut machine = Machine {
        executors,
        max_steps,
        signal: options.signal,
        steps: 0,
        scopes: vec![options.variables],
        tasks: Vec::new(),
        values: Vec::new(),
        results: Vec::new(),
    };
    match &root.kind {
        // A top-level block runs in the root scope, so its assignments are
        // the result's variables. It isn't a step of its own.
        InputKind::Do(list) => machine.enter_block(list, false),
        _ => machine.tasks.push(Task::Resolve(root)),
    }
    machine.drive().await?;
    Ok(RunResult {
        value: machine.values.pop().unwrap_or_default(),
        variables: machine.scopes.swap_remove(0),
        steps: machine.steps,
    })
}

/// What's left to do. Tasks that need a value find it on the value stack.
enum Task<'p> {
    /// Produce this input's value.
    Resolve(&'p Input),
    /// Run these instructions in order, in the current block.
    List(&'p [Instruction]),
    /// Run this instruction.
    Execute(&'p Instruction),
    /// Call the executor with the arguments on the value stack.
    Call(&'p Operation, &'p Input),
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
}

/// One run's state.
struct Machine<'p, 'e> {
    executors: &'e Executors,
    max_steps: u64,
    signal: Option<Signal>,
    steps: u64,
    /// Variables: a block pushes its own scope, reads fall through to the
    /// enclosing ones, and `set` writes the innermost.
    scopes: Vec<Variables>,
    tasks: Vec<Task<'p>>,
    values: Vec<Value>,
    /// What each running block will return (`null` until a `ret`).
    results: Vec<Value>,
}

impl<'p> Machine<'p, '_> {
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
            Task::List(list) => {
                if let Some((first, rest)) = list.split_first() {
                    if !rest.is_empty() {
                        self.tasks.push(Task::List(rest));
                    }
                    self.tasks.push(Task::Execute(first));
                }
            }
            Task::Execute(ins) => self.execute(ins)?,
            Task::Assign(name) => {
                let value = self.pop();
                self.scope().insert(name.to_owned(), value);
            }
            Task::Return => self.ret(),
            Task::Branch { then, otherwise } => {
                if truthy(&self.pop()) {
                    self.tasks.push(Task::List(then));
                } else if let Some(otherwise) = otherwise {
                    self.tasks.push(Task::List(otherwise));
                }
            }
            Task::Loop { test, body } => {
                if truthy(&self.pop()) {
                    self.tasks.push(Task::Loop { test, body });
                    self.tasks.push(Task::Resolve(test));
                    self.tasks.push(Task::List(body));
                }
            }
            Task::EndBlock { scoped } => {
                if scoped {
                    self.scopes.pop();
                }
                let result = self.results.pop().unwrap_or_default();
                self.values.push(result);
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
            InputKind::Op(operation) => {
                // Arguments resolve one at a time, in order: the first
                // pushed last, so it runs first.
                self.tasks.push(Task::Call(operation, input));
                for (_, argument) in operation.args.iter().rev() {
                    self.tasks.push(Task::Resolve(argument));
                }
                return Ok(());
            }
            InputKind::Do(list) => {
                self.enter_block(list, true);
                return Ok(());
            }
        };
        self.values.push(value);
        Ok(())
    }

    fn execute(&mut self, ins: &'p Instruction) -> Result<(), Error> {
        self.step(&ins.path, ins.tag.as_deref())?;
        match &ins.kind {
            InstructionKind::Set { name, to } => {
                self.tasks.push(Task::Assign(name));
                self.tasks.push(Task::Resolve(to));
            }
            InstructionKind::Ret(input) => {
                self.tasks.push(Task::Return);
                self.tasks.push(Task::Resolve(input));
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
                self.tasks.push(Task::Resolve(test));
            }
            InstructionKind::For { test, body } => {
                self.tasks.push(Task::Loop { test, body });
                self.tasks.push(Task::Resolve(test));
            }
        }
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
        while let Some(task) = self.tasks.last() {
            if matches!(task, Task::EndBlock { .. }) {
                break;
            }
            self.tasks.pop();
        }
    }

    async fn call(&mut self, operation: &Operation, node: &Input) -> Result<Value, Error> {
        let tag = node.tag.as_deref();
        let at = self.values.len().saturating_sub(operation.args.len());
        let values = self.values.split_off(at);
        let input: Map = operation
            .args
            .iter()
            .map(|(name, _)| name.clone())
            .zip(values)
            .collect();
        // Checked by validate() against this engine's own executors.
        let Some(executor) = self.executors.get(&operation.exe) else {
            let message = format!("Unknown executor \"{}\"", operation.exe);
            return Err(failure(
                ErrorCode::UnknownExecutor,
                message,
                &node.path,
                tag,
                None,
            ));
        };
        let call = ExecutorCall {
            op: operation.op.clone(),
            arg: operation.arg.clone(),
            input,
            signal: self.signal.clone(),
        };
        executor.call(call).await.map_err(|error| {
            match error.downcast::<Error>() {
                // An executor's own engine error passes through unchanged.
                Ok(own) => *own,
                Err(cause) => {
                    let message = format!(
                        "Executor \"{}\" failed on \"{}\": {cause}",
                        operation.exe, operation.op
                    );
                    let cause = Some(Cause::from(cause));
                    failure(ErrorCode::ExecutorFailed, message, &node.path, tag, cause)
                }
            }
        })
    }

    fn expression(&mut self, expression: &Expression, tag: Option<&str>) -> Result<Value, Error> {
        let mut cx = ExpressionContext {
            scopes: &self.scopes,
            steps: &mut self.steps,
            max_steps: self.max_steps,
            signal: self.signal.as_ref(),
            path: &expression.path,
            tag,
        };
        expression::evaluate(&expression.ast, &mut cx).map_err(|fault| {
            let (code, message) = match fault {
                Fault::Stop(error) => return error,
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
        step(
            &mut self.steps,
            self.max_steps,
            self.signal.as_ref(),
            path,
            tag,
        )
    }

    fn get(&self, name: &str) -> Option<&Value> {
        lookup(&self.scopes, name)
    }

    fn scope(&mut self) -> &mut Variables {
        // The root scope is never popped.
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
struct ExpressionContext<'v, 's> {
    scopes: &'v [Variables],
    steps: &'s mut u64,
    max_steps: u64,
    signal: Option<&'s Signal>,
    path: &'s str,
    tag: Option<&'s str>,
}

impl<'v> expression::Context<'v> for ExpressionContext<'v, '_> {
    // `null` is a value; only a missing variable is undefined.
    fn get(&self, name: &str) -> Option<&'v Value> {
        lookup(self.scopes, name)
    }

    fn step(&mut self) -> Result<(), Error> {
        step(self.steps, self.max_steps, self.signal, self.path, self.tag)
    }
}

/// Counts one step, failing once the signal is aborted or past `max_steps`.
fn step(
    steps: &mut u64,
    max_steps: u64,
    signal: Option<&Signal>,
    path: &str,
    tag: Option<&str>,
) -> Result<(), Error> {
    if let Some(signal) = signal.filter(|s| s.is_aborted()) {
        let reason = signal.reason();
        return Err(failure(
            ErrorCode::Aborted,
            "Run aborted",
            path,
            tag,
            reason,
        ));
    }
    *steps = steps.saturating_add(1);
    if *steps > max_steps {
        let message = format!("Program exceeded {max_steps} steps");
        return Err(failure(ErrorCode::StepLimit, message, path, tag, None));
    }
    Ok(())
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
