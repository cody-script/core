//! Running a compiled program.
//!
//! The interpreter is a machine with explicit stacks — tasks to do, values
//! produced, block results, scopes — driven by one loop. Nothing recurses, so
//! the stack a run uses doesn't grow with the program's depth, nor with how
//! deeply its functions call each other: neither can overflow it, natively or
//! under WebAssembly's small stack. The loop's only `.await`s are an executor
//! call and, in a debugged run, a pause.
//!
//! Every step is counted at the moment a recursive interpreter would count
//! it, and in the same order: an instruction or input is a step when it
//! starts, before its parts.
//!
//! Work that would be pushed and popped straight away is done at once
//! instead: a list runs its first instruction, and an instruction resolves
//! its input. Only those two calls are direct (resolving an input never runs
//! instructions itself; a block's are pushed as a task), so the native stack
//! stays the same depth however the program nests.
//!
//! A debugged run may pause before each step to ask its debugger how to go
//! on. The loop is built twice, with and without that check, so a run that
//! isn't debugged doesn't pay for it. A debugged run does nothing at once:
//! every step is a task of its own, so it can pause before any of them.

use std::sync::Arc;

use crate::debug::{Frame, Node, Session};
use crate::engine::{Limits, RunOptions, RunResult, Variables};
use crate::error::{Cause, Details, Error, ErrorCode};
use crate::executor::{ExecutorCall, Signal};
use crate::expression::{self, Ast, Fault, Stop};
use crate::program::{
    Call, Check, External, Function, Input, InputKind, Instruction, InstructionKind, Program,
};
use crate::types::{self, TypeDef};
use crate::value::{Map, Value};

/// Runs `entry` — `program`'s body, or one of its tests' — to completion,
/// in the root scope; with a debugger, pausing where it says. A test passes
/// if its run does.
pub(crate) async fn run(
    program: &Program,
    entry: &Input,
    limits: Limits,
    options: RunOptions<'_>,
) -> Result<RunResult, Error> {
    let RunOptions {
        variables,
        signal,
        breakpoints,
        debugger,
    } = options;
    let debug = match debugger {
        Some(debugger) => Some(Session::new(program, breakpoints, debugger)?),
        None => None,
    };
    let mut machine = Machine {
        functions: &program.functions,
        types: &program.types,
        main: &program.body,
        meter: Meter {
            steps: 0,
            max_steps: limits.max_steps,
            signal,
        },
        max_depth: limits.max_depth,
        scopes: vec![variables],
        base: 0,
        invocations: Vec::new(),
        tasks: Vec::new(),
        values: Vec::new(),
        results: Vec::new(),
        debug,
    };
    // The body (or the test) runs in the root scope, so its assignments are
    // the result's variables.
    machine.enter(entry);
    if machine.debug.is_some() {
        machine.drive::<true>().await?;
    } else {
        machine.drive::<false>().await?;
    }
    Ok(RunResult {
        value: machine.values.pop().unwrap_or_default(),
        variables: machine.scopes.swap_remove(0),
        steps: machine.meter.steps,
    })
}

/// What's left to do. Tasks that need a value find it on the value stack.
#[derive(Debug)]
enum Task<'p> {
    /// Produce this input's value.
    Resolve(&'p Input),
    /// Run these instructions in order, in the current block.
    List(&'p [Instruction]),
    /// Run this instruction. Only a debugged run has these: other runs
    /// execute an instruction at once.
    Execute(&'p Instruction),
    /// Call the external function with the arguments on the value stack.
    External(&'p External, &'p Input),
    /// Run the function with the arguments on the value stack as its
    /// variables.
    Invoke(&'p Call, &'p Input),
    /// Make an object of these fields, whose values are on the value stack.
    Object(&'p [(Arc<str>, Input)]),
    /// Make a list of the last this many values on the value stack.
    Array(usize),
    /// Check the value against a type: `is` or `problems`.
    Check(&'p Check, &'p Input),
    /// Run the program's body with the variables on the value stack: a
    /// test's `run`.
    RunProgram(&'p [(Arc<str>, Input)], &'p Input),
    /// Fail unless the value is `true`, or (with `eq`) the two values are
    /// equal.
    Expect(&'p Instruction),
    /// Assign the value to this variable in the current scope.
    Assign(&'p Arc<str>),
    /// End the current block with the value.
    Return,
    /// Run `then` if the value (`test`'s) is `true`, else `otherwise`.
    Branch {
        test: &'p Input,
        then: &'p [Instruction],
        otherwise: Option<&'p [Instruction]>,
    },
    /// If the value (the loop's test) is `true`, run `body` and test again.
    Loop {
        test: &'p Input,
        body: &'p [Instruction],
    },
    /// The current block is over: its result becomes a value, and its
    /// scope, if it has its own, goes.
    EndBlock { scoped: bool },
    /// The current function is over: its value stays, its scopes go, and
    /// the caller's are visible again.
    EndCall,
}

impl<'p> Task<'p> {
    /// The step this task takes, if it's one: a debugger may pause before it.
    fn step(&self) -> Option<Node<'p>> {
        match *self {
            Task::Resolve(input) => Some(Node::Input(input)),
            Task::Execute(ins) => Some(Node::Instruction(ins)),
            _ => None,
        }
    }
}

/// A CodyScript function running — or, from a test's `run`, the program's
/// body: a frame of the call stack.
#[derive(Debug)]
struct Invocation<'p> {
    /// The `call` (or `run`) running it.
    call: &'p Input,
    /// The function; `None` for the program's body.
    function: Option<&'p Function>,
    /// Where its scopes start.
    base: usize,
}

/// The call stack, as a debugger sees it: the program's body, then each
/// function running, each with its own scopes.
fn frames<'a>(scopes: &'a [Variables], invocations: &'a [Invocation<'_>]) -> Vec<Frame<'a>> {
    let mut frames = Vec::with_capacity(invocations.len() + 1);
    let mut end = scopes.len();
    for invocation in invocations.iter().rev() {
        frames.push(Frame {
            function: invocation.function.map(|f| f.name.as_str()),
            call: Some(&invocation.call.path),
            scopes: &scopes[invocation.base..end],
        });
        end = invocation.base;
    }
    frames.push(Frame {
        function: None,
        call: None,
        scopes: &scopes[..end],
    });
    frames.reverse();
    frames
}

/// One run's state.
struct Machine<'p, 'd> {
    /// The program's functions, by index.
    functions: &'p [Function],
    /// The types it declares, by index.
    types: &'p [TypeDef],
    /// Its body: what a test's `run` runs.
    main: &'p Input,
    meter: Meter,
    /// How deeply calls may nest.
    max_depth: usize,
    /// Variables: a block pushes its own scope, reads fall through to the
    /// enclosing ones, and `set` writes the innermost.
    scopes: Vec<Variables>,
    /// Where the running function's scopes start: reads don't fall through
    /// below it, so a function sees only its own variables.
    base: usize,
    /// The calls running, outermost first; how many decides `depth_limit`.
    invocations: Vec<Invocation<'p>>,
    tasks: Vec<Task<'p>>,
    values: Vec<Value>,
    /// What each running block will return (`null` until a `ret`).
    results: Vec<Value>,
    /// The debugger, in a debugged run.
    debug: Option<Session<'p, 'd>>,
}

impl<'p> Machine<'p, '_> {
    /// Does every task. `DEBUGGED` (whether `debug` is set) builds the loop
    /// twice, so that only a debugged run has the chance to pause.
    async fn drive<const DEBUGGED: bool>(&mut self) -> Result<(), Error> {
        while let Some(task) = self.tasks.pop() {
            if let Task::External(external, node) = task {
                let value = self.call_external(external, node).await?;
                self.values.push(value);
            } else if DEBUGGED {
                self.debug_task(task).await?;
            } else {
                self.perform::<false>(task)?;
            }
        }
        Ok(())
    }

    /// Does `task` in a debugged run — first, if it's a step, letting the
    /// debugging session pause the run, and stop it.
    async fn debug_task(&mut self, task: Task<'p>) -> Result<(), Error> {
        if let Some(step) = task.step()
            && let Some(session) = self.debug.as_mut()
        {
            let frames = || frames(&self.scopes, &self.invocations);
            let depth = self.invocations.len();
            session
                .before(step, depth, self.meter.steps, frames)
                .await?;
        }
        self.perform::<true>(task)
    }

    /// Does one task that needs no waiting. In a debugged run (`DEBUGGED`),
    /// it pushes the steps it would otherwise take at once.
    fn perform<const DEBUGGED: bool>(&mut self, task: Task<'p>) -> Result<(), Error> {
        match task {
            Task::Resolve(input) => self.resolve(input)?,
            Task::List(list) => self.list::<DEBUGGED>(list)?,
            Task::Execute(ins) => self.execute::<DEBUGGED>(ins)?,
            Task::Assign(name) => {
                let value = self.pop();
                // Reassigning, the usual case in a loop, needs no new name;
                // a new variable shares the program's.
                let scope = self.scope();
                match scope.get_mut(name) {
                    Some(slot) => *slot = value,
                    None => {
                        scope.insert(Arc::clone(name), value);
                    }
                }
            }
            Task::Return => self.ret(),
            Task::Branch {
                test,
                then,
                otherwise,
            } => {
                if self.test("if", test)? {
                    self.list::<DEBUGGED>(then)?;
                } else if let Some(otherwise) = otherwise {
                    self.list::<DEBUGGED>(otherwise)?;
                }
            }
            Task::Loop { test, body } => {
                if self.test("for", test)? {
                    self.tasks.push(Task::Loop { test, body });
                    self.tasks.push(Task::Resolve(test));
                    self.list::<DEBUGGED>(body)?;
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
            Task::Object(fields) => {
                let object = self.arguments(fields);
                self.values.push(Value::Object(object));
            }
            Task::Array(len) => {
                let at = self.values.len().saturating_sub(len);
                let items = self.values.drain(at..).collect();
                self.values.push(Value::Array(items));
            }
            Task::EndCall => {
                self.scopes.truncate(self.base);
                self.invocations.pop();
                self.base = self.invocations.last().map_or(0, |i| i.base);
            }
            Task::Check(check, node) => self.check(check, node)?,
            Task::RunProgram(variables, node) => {
                self.enter_call(node, None, variables, self.main)?;
            }
            Task::Expect(ins) => self.expect(ins)?,
            Task::External(..) => unreachable!("external calls are awaited by drive()"),
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
            InputKind::X(ast) => self.expression(ast, input)?,
            InputKind::External(external) => {
                // Arguments resolve one at a time, in order: the first
                // pushed last, so it runs first.
                self.tasks.push(Task::External(external, input));
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
            InputKind::Obj(fields) => {
                // Like arguments: each field's value in turn, in order.
                self.tasks.push(Task::Object(fields));
                for (_, field) in fields.iter().rev() {
                    self.tasks.push(Task::Resolve(field));
                }
                return Ok(());
            }
            InputKind::List(items) => {
                self.tasks.push(Task::Array(items.len()));
                for item in items.iter().rev() {
                    self.tasks.push(Task::Resolve(item));
                }
                return Ok(());
            }
            InputKind::Check(check) => {
                self.tasks.push(Task::Check(check, input));
                self.tasks.push(Task::Resolve(&check.value));
                return Ok(());
            }
            InputKind::Run(variables) => {
                // Like a call's arguments: each variable's value in turn.
                self.tasks.push(Task::RunProgram(variables, input));
                for (_, variable) in variables.iter().rev() {
                    self.tasks.push(Task::Resolve(variable));
                }
                return Ok(());
            }
        };
        self.values.push(value);
        Ok(())
    }

    /// Runs `list`'s first instruction now (in a debugged run, next), and
    /// the rest after it.
    fn list<const DEBUGGED: bool>(&mut self, list: &'p [Instruction]) -> Result<(), Error> {
        let Some((first, rest)) = list.split_first() else {
            return Ok(());
        };
        if !rest.is_empty() {
            self.tasks.push(Task::List(rest));
        }
        if DEBUGGED {
            self.tasks.push(Task::Execute(first));
            return Ok(());
        }
        self.execute::<false>(first)
    }

    /// Runs an instruction: resolves its input now (in a debugged run,
    /// next), and pushes what to do with the value.
    fn execute<const DEBUGGED: bool>(&mut self, ins: &'p Instruction) -> Result<(), Error> {
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
                    test,
                    then,
                    otherwise: otherwise.as_deref(),
                });
                test
            }
            InstructionKind::For { test, body } => {
                self.tasks.push(Task::Loop { test, body });
                test
            }
            InstructionKind::Expect(expect) => {
                // The value, then `eq`'s: the last pushed runs first.
                self.tasks.push(Task::Expect(ins));
                if let Some(eq) = &expect.eq {
                    self.tasks.push(Task::Resolve(eq));
                }
                &expect.value
            }
        };
        if DEBUGGED {
            self.tasks.push(Task::Resolve(input));
            return Ok(());
        }
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
        let function = &self.functions[call.function];
        self.enter_call(node, Some(function), &call.args, &function.body)
    }

    /// Runs `body` — a function's, or the program's — as a call from `node`
    /// (`function` names it): the values of `args`, on the value stack,
    /// become the variables of a fresh scope that the caller's can't be read
    /// through, and `body` runs there until `EndCall`.
    fn enter_call(
        &mut self,
        node: &'p Input,
        function: Option<&'p Function>,
        args: &'p [(Arc<str>, Input)],
        body: &'p Input,
    ) -> Result<(), Error> {
        if self.invocations.len() >= self.max_depth {
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
        let variables = self.arguments(args);
        self.tasks.push(Task::EndCall);
        self.base = self.scopes.len();
        self.invocations.push(Invocation {
            call: node,
            function,
            base: self.base,
        });
        self.scopes.push(variables);
        self.enter(body);
        Ok(())
    }

    /// `is` or `problems`: the value checked against the type, a step for
    /// each value looked at. Not inlined, like `expect`: kept out of the
    /// loop, which every run pays for.
    #[inline(never)]
    fn check(&mut self, check: &'p Check, node: &'p Input) -> Result<(), Error> {
        let value = self.pop();
        let meter = &mut self.meter;
        let found = types::problems(self.types, &value, &check.ty, || meter.step())
            .map_err(|stop| self.meter.error(stop, &node.path, node.tag.as_deref()))?;
        self.values.push(if check.problems {
            Value::Array(found.into_iter().map(types::as_value).collect())
        } else {
            Value::Bool(found.is_empty())
        });
        Ok(())
    }

    /// `expect`: the value (and with `eq`, `eq`'s, on top) must be `true` —
    /// or equal, as values are: lists item by item, objects field by field
    /// in any order, numbers by value.
    #[inline(never)]
    fn expect(&mut self, ins: &'p Instruction) -> Result<(), Error> {
        let InstructionKind::Expect(expect) = &ins.kind else {
            unreachable!("an expect task is an expect instruction's");
        };
        let (input, eq) = (&expect.value, &expect.eq);
        let tag = ins.tag.as_deref();
        let (expected, value) = match eq {
            Some(_) => {
                let expected = self.pop();
                (expected, self.pop())
            }
            None => match self.pop() {
                Value::Bool(b) => (Value::Bool(true), Value::Bool(b)),
                other => {
                    let message = format!(
                        "\"expect\" needs a boolean, or \"eq\" to compare with, got {}",
                        expression::kind(&other)
                    );
                    let path = &input.path;
                    return Err(failure(
                        ErrorCode::TypeError,
                        message,
                        path,
                        input.tag.as_deref(),
                        None,
                    ));
                }
            },
        };
        if value == expected {
            return Ok(());
        }
        let message = format!("Expected {}, got {}", shown(&expected), shown(&value));
        Err(failure(
            ErrorCode::ExpectationFailed,
            message,
            &ins.path,
            tag,
            None,
        ))
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

    /// The arguments (or an `obj`'s fields) on the value stack, by name,
    /// taken off it. (The names are distinct: they're an object's keys.)
    fn arguments(&mut self, args: &[(Arc<str>, Input)]) -> Map {
        let at = self.values.len().saturating_sub(args.len());
        let names = args.iter().map(|(name, _)| Arc::clone(name));
        Map::from_unique(names.zip(self.values.drain(at..)).collect())
    }

    /// Calls an external function — the host's — with the arguments on the
    /// value stack.
    async fn call_external(&mut self, external: &External, node: &Input) -> Result<Value, Error> {
        let tag = node.tag.as_deref();
        let call = ExecutorCall {
            op: Arc::clone(&external.op),
            arg: external.arg.clone(),
            input: self.arguments(&external.args),
            signal: self.meter.signal.clone(),
            tag: external.tag.clone(),
        };
        let name = &external.name;
        external.executor.call(call).await.map_err(|error| {
            match error.downcast::<Error>() {
                // An executor's own engine error passes through unchanged.
                Ok(own) => *own,
                Err(cause) => {
                    let message = format!("Function \"{name}\" failed: {cause}");
                    let cause = Some(Cause::from(cause));
                    failure(ErrorCode::FunctionFailed, message, &node.path, tag, cause)
                }
            }
        })
    }

    /// Evaluates the expression of `input`, an `x` node. Its failures point
    /// at the node's `/x`.
    fn expression(&mut self, ast: &Ast, input: &Input) -> Result<Value, Error> {
        let mut cx = ExpressionContext {
            scopes: &self.scopes[self.base..],
            meter: &mut self.meter,
        };
        expression::evaluate(ast, &mut cx).map_err(|fault| {
            let (path, tag) = (format!("{}/x", input.path), input.tag.as_deref());
            let (code, message) = match fault {
                Fault::Stop(stop) => return self.meter.error(stop, &path, tag),
                Fault::Undefined(name) => (
                    ErrorCode::UndefinedVariable,
                    format!("Variable \"{name}\" is not defined"),
                ),
                Fault::Type(message) => (ErrorCode::TypeError, message),
            };
            let message = format!("{message} in \"{}\"", ast.source);
            failure(code, message, &path, tag, None)
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

    /// The value of `instruction`'s `test`, which must be a boolean.
    fn test(&mut self, instruction: &str, test: &Input) -> Result<bool, Error> {
        match self.pop() {
            Value::Bool(b) => Ok(b),
            other => {
                let message = format!(
                    "\"{instruction}\" needs a boolean, got {}",
                    expression::kind(&other)
                );
                let tag = test.tag.as_deref();
                Err(failure(
                    ErrorCode::TypeError,
                    message,
                    &test.path,
                    tag,
                    None,
                ))
            }
        }
    }

    /// The value a task needs. The machine always pushes one first.
    fn pop(&mut self) -> Value {
        self.values.pop().unwrap_or_default()
    }
}

/// How much of a value a failed `expect` shows, at most.
const SHOWN: usize = 120;

/// `value` written out for a message, in JSON's notation, cut short past
/// [`SHOWN`] characters. Walks an explicit stack, and stops once it's long
/// enough, so no value is too deep or too big to show.
fn shown(value: &Value) -> String {
    use std::fmt::Write;
    enum Part<'v> {
        Value(&'v Value),
        Text(&'static str),
        Key(&'v str),
    }
    let mut out = String::new();
    let mut parts = vec![Part::Value(value)];
    while let Some(part) = parts.pop() {
        if out.len() > SHOWN {
            break;
        }
        match part {
            Part::Text(text) => out.push_str(text),
            Part::Key(key) => {
                let _ = write!(out, "{key:?}: ");
            }
            Part::Value(Value::Null) => out.push_str("null"),
            Part::Value(Value::Bool(b)) => {
                let _ = write!(out, "{b}");
            }
            Part::Value(Value::Number(n)) => {
                let _ = write!(out, "{n}");
            }
            Part::Value(Value::String(s)) => {
                let _ = write!(out, "{s:?}");
            }
            Part::Value(Value::Array(items)) => {
                parts.push(Part::Text("]"));
                for (i, item) in items.iter().enumerate().rev() {
                    parts.push(Part::Value(item));
                    if i > 0 {
                        parts.push(Part::Text(", "));
                    }
                }
                out.push('[');
            }
            Part::Value(Value::Object(map)) => {
                parts.push(Part::Text("}"));
                let entries: Vec<_> = map.iter().collect();
                for (i, (key, item)) in entries.into_iter().enumerate().rev() {
                    parts.push(Part::Value(item));
                    parts.push(Part::Key(key));
                    if i > 0 {
                        parts.push(Part::Text(", "));
                    }
                }
                out.push('{');
            }
        }
    }
    if out.len() > SHOWN || !parts.is_empty() {
        let end = (0..=SHOWN.min(out.len()))
            .rev()
            .find(|&i| out.is_char_boundary(i))
            .unwrap_or(0);
        out.truncate(end);
        out.push('…');
    }
    out
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
