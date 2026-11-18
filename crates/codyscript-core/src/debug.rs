//! Debugging a run: pausing at breakpoints, looking at the variables, and
//! going on a step at a time. A run is debugged when its options have a
//! debugger: see [`RunOptions::with_debugger`](crate::RunOptions::with_debugger).
//!
//! A **step** is a node of the program: an instruction (`set`, `ret`, `if`,
//! `for`) or an input (a literal, `val`, `ref`, `x`, `call`, `do`) — what
//! a run counts as steps, except the operands inside an expression, which
//! run as part of their `x`. A step is named by its JSON Pointer into the
//! program (`/do/1`), the path errors use; a
//! [`CompiledProgram::steps`](crate::CompiledProgram::steps) lists them.
//!
//! A debugged run pauses only before a step a [`Breakpoint`] names — by its
//! tag, or its path — and then wherever the [`Action`] it goes on with says:
//! nowhere else, so with no breakpoints it runs to the end. Each time, it
//! hands the [`Debugger`] a [`Pause`] — where it is, the variables, the call
//! stack, and the breakpoints, which it may change — and waits for the
//! action. A breakpoint that names no step is an error
//! ([`ErrorCode::InvalidConfig`]), when the run starts or when it goes on.
//!
//! [`ErrorCode::InvalidConfig`]: crate::ErrorCode::InvalidConfig

use std::fmt;
use std::future::ready;

use serde::{Deserialize, Serialize};

use crate::engine::Variables;
use crate::error::{Details, Error, ErrorCode, Issue};
use crate::executor::BoxFuture;
use crate::program::{Function, Input, InputKind, Instruction, InstructionKind, Program};
use crate::value::Value;

/// A step to pause before, named by its tag or by its path.
///
/// As data, `{ "tag": "name-required" }` or `{ "path": "/do/1" }`. Prefer
/// the tag, where a step has one: a path changes when the program around
/// the step is edited (insert an instruction before `/do/1`, and it's
/// `/do/2`), but a tag moves with its node. A tag names every step that has
/// it. [`Step::breakpoint`] makes the best breakpoint for a step.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Breakpoint {
    /// The step at this path: a JSON Pointer into the program, as errors
    /// use.
    Path(String),
    /// Every step with this `tag`.
    Tag(String),
}

impl Breakpoint {
    /// The step at `path`.
    pub fn path(path: impl Into<String>) -> Self {
        Breakpoint::Path(path.into())
    }

    /// Every step tagged `tag`.
    pub fn tag(tag: impl Into<String>) -> Self {
        Breakpoint::Tag(tag.into())
    }

    /// Whether it names `step`.
    pub fn matches(&self, step: &Step<'_>) -> bool {
        self.names(step.path, step.tag)
    }

    /// Whether it names the step at `path`, tagged `tag`.
    fn names(&self, path: &str, tag: Option<&str>) -> bool {
        match self {
            Breakpoint::Path(at) => at == path,
            Breakpoint::Tag(name) => tag == Some(name),
        }
    }
}

/// Checks that each of `breakpoints` names one of `steps`: if not, an
/// `invalid_config` error listing every one that doesn't, by its place in
/// the run configuration (`/breakpoints/0`).
pub(crate) fn check(steps: &[Step<'_>], breakpoints: &[Breakpoint]) -> Result<(), Error> {
    let issues: Vec<Issue> = breakpoints
        .iter()
        .enumerate()
        .filter(|(_, breakpoint)| !steps.iter().any(|step| breakpoint.matches(step)))
        .map(|(i, breakpoint)| Issue {
            path: format!("/breakpoints/{i}"),
            message: match breakpoint {
                Breakpoint::Path(path) => format!("No step at \"{path}\""),
                Breakpoint::Tag(tag) => format!("No step has tag \"{tag}\""),
            },
        })
        .collect();
    let Some(first) = issues.first() else {
        return Ok(());
    };
    let message = format!("Invalid run configuration: {}", first.message);
    let details = Details {
        path: Some(first.path.clone()),
        issues: Some(issues),
        ..Details::default()
    };
    Err(Error::with_details(
        ErrorCode::InvalidConfig,
        message,
        details,
    ))
}

/// How a paused run goes on. As data, `"step_over"`, `"step_into"`,
/// `"continue"` or `"stop"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Take this step, with everything it involves — its parts, and the
    /// whole of any function it calls — and pause at the next one.
    StepOver,
    /// Pause at the very next step: this one's first part, or the first step
    /// of the function it calls (after its arguments); where it has neither,
    /// the next step, as for [`StepOver`](Action::StepOver).
    StepInto,
    /// Run until the next breakpoint, or the end.
    Continue,
    /// End the run here: it fails with [`ErrorCode::Aborted`] at this step,
    /// which isn't taken.
    ///
    /// [`ErrorCode::Aborted`]: crate::ErrorCode::Aborted
    Stop,
}

/// Why a run paused. As data, `"breakpoint"` or `"step"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// The step has a breakpoint.
    Breakpoint,
    /// Stepping ([`Action::StepOver`] or [`Action::StepInto`]) ended here.
    Step,
}

/// What a step is: the key that names its node's type, as written in a
/// program. As data, that key: `"set"`, `"call"`, …
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// A literal, or `val`: a constant.
    Val,
    /// `ref`: reads a variable.
    Ref,
    /// `x`: an expression.
    X,
    /// `call`: calls a function, the host's or the program's.
    Call,
    /// `do` as an input: a block with its own scope.
    Do,
    /// `set`: assigns a variable.
    Set,
    /// `ret`: ends the block with a value.
    Ret,
    /// `if`: a condition.
    If,
    /// `for`: a loop. A loop's test, a step of its own, runs before each pass.
    For,
}

impl NodeKind {
    /// The key, as written in a program: `"set"`, `"call"`, …
    pub const fn as_str(self) -> &'static str {
        match self {
            NodeKind::Val => "val",
            NodeKind::Ref => "ref",
            NodeKind::X => "x",
            NodeKind::Call => "call",
            NodeKind::Do => "do",
            NodeKind::Set => "set",
            NodeKind::Ret => "ret",
            NodeKind::If => "if",
            NodeKind::For => "for",
        }
    }
}

impl fmt::Display for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Debuggers ───────────────────────────────────────────────────────────────

/// Host code that decides, each time a debugged run pauses, how it goes on:
/// a person at a user interface, typically, or a script.
///
/// The run waits for the future `pause` returns, which may take as long as
/// it likes (until a person clicks a button, say); it may keep the
/// [`Pause`], to look at the variables or change the breakpoints as it
/// waits. For a debugger that answers at once, [`from_fn`] takes a plain
/// closure.
pub trait Debugger: Send {
    /// The run has paused, before the step `pause` describes; resolves to
    /// how it goes on.
    fn pause<'a>(&'a mut self, pause: Pause<'a>) -> BoxFuture<'a, Action>;
}

impl<D: Debugger + ?Sized> Debugger for &mut D {
    fn pause<'a>(&'a mut self, pause: Pause<'a>) -> BoxFuture<'a, Action> {
        (**self).pause(pause)
    }
}

impl<D: Debugger + ?Sized> Debugger for Box<D> {
    fn pause<'a>(&'a mut self, pause: Pause<'a>) -> BoxFuture<'a, Action> {
        (**self).pause(pause)
    }
}

/// A [`Debugger`] made from a closure; see [`from_fn`].
#[derive(Clone)]
pub struct FnDebugger<F>(F);

/// A debugger from a closure that answers each pause at once, and may change
/// the breakpoints as it does.
///
/// ```
/// use codyscript_core::debug::{self, Action, Breakpoint};
/// use codyscript_core::{value, Engine, RunOptions};
///
/// let program = Engine::new().compile(&value!({ "do": [
///     { "set": { "a": 1 } },
///     { "set": { "b": { "x": "a + 1" } } },
///     { "ret": { "x": "a + b" } }
/// ] })).unwrap();
/// let mut seen = Vec::new();
/// let debugger = debug::from_fn(|pause| {
///     seen.push((pause.step().path.to_owned(), pause.variable("b").cloned()));
///     Action::Continue
/// });
/// let breakpoints = [Breakpoint::path("/do/2")];
/// let options = RunOptions::new().with_debugger(breakpoints, debugger);
/// let result = program.run_blocking(options).unwrap();
/// assert_eq!(result.value, 3);
/// assert_eq!(seen, [("/do/2".to_owned(), Some(value!(2)))]);
/// ```
pub fn from_fn<F>(f: F) -> FnDebugger<F>
where
    F: FnMut(&mut Pause<'_>) -> Action + Send,
{
    FnDebugger(f)
}

impl<F> Debugger for FnDebugger<F>
where
    F: FnMut(&mut Pause<'_>) -> Action + Send,
{
    fn pause<'a>(&'a mut self, mut pause: Pause<'a>) -> BoxFuture<'a, Action> {
        Box::pin(ready((self.0)(&mut pause)))
    }
}

impl<F> fmt::Debug for FnDebugger<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FnDebugger")
    }
}

// ── What a debugger sees ────────────────────────────────────────────────────

/// A step of a program: a node a run can pause before. See
/// [`CompiledProgram::steps`](crate::CompiledProgram::steps).
///
/// Serializes as `{ path, kind, name?, tag?, function?, depth }`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Step<'a> {
    /// JSON Pointer to the node: what a breakpoint names.
    pub path: &'a str,
    /// Its type.
    pub kind: NodeKind,
    /// What it names, if anything: a `set`'s or `ref`'s variable, a `call`'s
    /// function, an `x`'s source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'a str>,
    /// Its `tag`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<&'a str>,
    /// The CodyScript function it's part of; `None` for the program's body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<&'a str>,
    /// How many steps it's inside, within the program's body or its
    /// function's: `0` for the top-level instructions (or a body that isn't
    /// a block), `1` for their parts, and so on.
    pub depth: usize,
}

impl Step<'_> {
    /// The breakpoint that names this step most durably: by its tag, if it
    /// has one (which then names every step with that tag), else by its
    /// path.
    pub fn breakpoint(&self) -> Breakpoint {
        match self.tag {
            Some(tag) => Breakpoint::tag(tag),
            None => Breakpoint::path(self.path),
        }
    }
}

/// One function running, in a [`Pause`]'s call stack.
///
/// Serializes as `{ function?, call?, scopes }`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Frame<'a> {
    /// The CodyScript function's name; `None` for the program's body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<&'a str>,
    /// The path of the `call` that's running it, in the frame before;
    /// `None` for the program's body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call: Option<&'a str>,
    /// Its variables: its scopes, outermost first — the run's variables or
    /// the function's arguments, then one for each `do` block it's inside.
    /// A step reads a variable from the innermost scope that has it.
    pub scopes: &'a [Variables],
}

/// A paused run, as its [`Debugger`] sees it: the step it's about to take,
/// its call stack and variables, and the breakpoints.
///
/// Serializes as `{ reason, path, kind, name?, tag?, function?, depth,
/// steps, frames, breakpoints }` — the step's fields flattened in: what a
/// host might send a user interface.
#[derive(Debug, Serialize)]
pub struct Pause<'a> {
    reason: Reason,
    #[serde(flatten)]
    step: Step<'a>,
    steps: u64,
    frames: Vec<Frame<'a>>,
    breakpoints: &'a mut Vec<Breakpoint>,
}

impl<'a> Pause<'a> {
    /// Why the run paused here.
    pub fn reason(&self) -> Reason {
        self.reason
    }

    /// The step it's about to take, as
    /// [`CompiledProgram::steps`](crate::CompiledProgram::steps) lists it.
    pub fn step(&self) -> Step<'a> {
        self.step
    }

    /// Steps taken so far, not counting this one.
    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// The call stack: the program's body first, then each CodyScript
    /// function running, the step's own last.
    pub fn frames(&self) -> &[Frame<'a>] {
        &self.frames
    }

    /// The variables the step can see: its own frame's scopes, outermost
    /// first.
    pub fn scopes(&self) -> &'a [Variables] {
        // There's always the program's body's frame.
        self.frames.last().map_or(&[], |frame| frame.scopes)
    }

    /// A variable, as the step would read it: from the innermost of
    /// [`scopes`](Pause::scopes) that has it.
    pub fn variable(&self, name: &str) -> Option<&'a Value> {
        self.scopes().iter().rev().find_map(|scope| scope.get(name))
    }

    /// The breakpoints.
    pub fn breakpoints(&self) -> &[Breakpoint] {
        self.breakpoints
    }

    /// The breakpoints, to change: the run goes on with them. Each must
    /// name a step, or the run fails with `invalid_config` when it goes on.
    pub fn breakpoints_mut(&mut self) -> &mut Vec<Breakpoint> {
        self.breakpoints
    }
}

// ── Where to pause ──────────────────────────────────────────────────────────

/// A debugged run's side of debugging: its debugger and breakpoints, and
/// where stepping is to pause it next. The run asks it, before each step,
/// whether to go on.
pub(crate) struct Session<'p, 'd> {
    debugger: Box<dyn Debugger + 'd>,
    breakpoints: Vec<Breakpoint>,
    stepping: Stepping<'p>,
    /// The program's steps, for pauses to show and breakpoints to name.
    steps: Vec<Step<'p>>,
}

/// Where to pause, besides at breakpoints.
#[derive(Clone, Copy)]
enum Stepping<'p> {
    /// Nowhere else.
    No,
    /// At the next step.
    Into,
    /// At the next step that isn't part of the one at `path` (nor of a
    /// function it calls), `depth` calls deep: once it's over.
    Over { path: &'p str, depth: usize },
}

impl<'p, 'd> Session<'p, 'd> {
    /// A session for debugging `program`; fails if a breakpoint names no
    /// step of it.
    pub fn new(
        program: &'p Program,
        breakpoints: Vec<Breakpoint>,
        debugger: Box<dyn Debugger + 'd>,
    ) -> Result<Self, Error> {
        let steps = steps(program);
        check(&steps, &breakpoints)?;
        Ok(Session {
            debugger,
            breakpoints,
            stepping: Stepping::No,
            steps,
        })
    }

    /// Before `node`, a step taken `depth` calls deep with `taken` steps
    /// taken: if a breakpoint names it or stepping ends there, pauses —
    /// showing the debugger the call stack `frames` makes — and goes on as
    /// the debugger says. Fails if it says to stop, or leaves a breakpoint
    /// that names no step.
    pub async fn before<'s>(
        &'s mut self,
        node: Node<'p>,
        depth: usize,
        taken: u64,
        frames: impl FnOnce() -> Vec<Frame<'s>>,
    ) -> Result<(), Error> {
        let (path, tag) = (node.path(), node.tag());
        let Some(reason) = self.reason(path, tag, depth) else {
            return Ok(());
        };
        let pause = Pause {
            reason,
            // Every node a run steps through is listed.
            step: *self
                .steps
                .iter()
                .find(|step| step.path == path)
                .expect("a listed step"),
            steps: taken,
            frames: frames(),
            breakpoints: &mut self.breakpoints,
        };
        self.stepping = match self.debugger.pause(pause).await {
            Action::StepInto => Stepping::Into,
            Action::StepOver => Stepping::Over { path, depth },
            Action::Continue => Stepping::No,
            Action::Stop => {
                let details = Details {
                    path: Some(path.to_owned()),
                    tag: tag.map(str::to_owned),
                    ..Details::default()
                };
                let message = "Run stopped by the debugger";
                return Err(Error::with_details(ErrorCode::Aborted, message, details));
            }
        };
        check(&self.steps, &self.breakpoints)
    }

    /// Why to pause before the step at `path`, tagged `tag`, taken `depth`
    /// calls deep, if at all.
    fn reason(&self, path: &str, tag: Option<&str>, depth: usize) -> Option<Reason> {
        if self.breakpoints.iter().any(|b| b.names(path, tag)) {
            return Some(Reason::Breakpoint);
        }
        let stepped = match self.stepping {
            Stepping::No => false,
            Stepping::Into => true,
            Stepping::Over {
                path: over,
                depth: at,
            } => depth < at || (depth == at && !inside(path, over)),
        };
        stepped.then_some(Reason::Step)
    }
}

/// Whether the node at `path` is part of the one at `ancestor`.
fn inside(path: &str, ancestor: &str) -> bool {
    path.strip_prefix(ancestor)
        .is_some_and(|rest| rest.starts_with('/'))
}

// ── Steps ───────────────────────────────────────────────────────────────────

/// A step, or a part of one, as the run holds it.
#[derive(Clone, Copy)]
pub(crate) enum Node<'p> {
    Input(&'p Input),
    Instruction(&'p Instruction),
}

impl<'p> Node<'p> {
    pub(crate) fn path(self) -> &'p str {
        match self {
            Node::Input(input) => &input.path,
            Node::Instruction(ins) => &ins.path,
        }
    }

    pub(crate) fn tag(self) -> Option<&'p str> {
        match self {
            Node::Input(input) => input.tag.as_deref(),
            Node::Instruction(ins) => ins.tag.as_deref(),
        }
    }

    fn kind(self) -> NodeKind {
        match self {
            Node::Input(input) => match &input.kind {
                InputKind::Const(_) => NodeKind::Val,
                InputKind::Ref(_) => NodeKind::Ref,
                InputKind::X(_) => NodeKind::X,
                InputKind::External(_) | InputKind::Call(_) => NodeKind::Call,
                InputKind::Do(_) => NodeKind::Do,
            },
            Node::Instruction(ins) => match &ins.kind {
                InstructionKind::Set { .. } => NodeKind::Set,
                InstructionKind::Ret(_) => NodeKind::Ret,
                InstructionKind::If { .. } => NodeKind::If,
                InstructionKind::For { .. } => NodeKind::For,
            },
        }
    }

    fn name(self, functions: &'p [Function]) -> Option<&'p str> {
        match self {
            Node::Input(input) => match &input.kind {
                InputKind::Ref(name) => Some(name),
                InputKind::X(ast) => Some(&ast.source),
                InputKind::External(external) => Some(&external.name),
                InputKind::Call(call) => Some(&functions[call.function].name),
                InputKind::Const(_) | InputKind::Do(_) => None,
            },
            Node::Instruction(ins) => match &ins.kind {
                InstructionKind::Set { name, .. } => Some(name),
                _ => None,
            },
        }
    }
}

/// Every step of `program`, in the order written: its body's, then each
/// function's. Walks an explicit stack, so no program can overflow this one.
pub(crate) fn steps(program: &Program) -> Vec<Step<'_>> {
    let functions = &program.functions;
    let bodies = std::iter::once((None, &program.body))
        .chain(functions.iter().map(|f| (Some(f.name.as_str()), &f.body)));
    let mut steps = Vec::new();
    let mut stack = Vec::new();
    for (function, body) in bodies {
        // A body that is a block isn't a step of its own; its instructions
        // are.
        match &body.kind {
            InputKind::Do(list) => push_all(&mut stack, list, 0),
            _ => stack.push((Node::Input(body), 0)),
        }
        while let Some((node, depth)) = stack.pop() {
            steps.push(Step {
                path: node.path(),
                kind: node.kind(),
                name: node.name(functions),
                tag: node.tag(),
                function,
                depth,
            });
            push_parts(&mut stack, node, depth + 1);
        }
    }
    steps
}

/// Pushes `node`'s parts that are steps, `depth` deep. Everything goes on
/// last first, so it comes off in the order it runs.
fn push_parts<'p>(stack: &mut Vec<(Node<'p>, usize)>, node: Node<'p>, depth: usize) {
    match node {
        Node::Input(input) => match &input.kind {
            InputKind::External(external) => push_args(stack, &external.args, depth),
            InputKind::Call(call) => push_args(stack, &call.args, depth),
            InputKind::Do(list) => push_all(stack, list, depth),
            InputKind::Const(_) | InputKind::Ref(_) | InputKind::X(_) => {}
        },
        Node::Instruction(ins) => match &ins.kind {
            InstructionKind::Set { to: input, .. } | InstructionKind::Ret(input) => {
                stack.push((Node::Input(input), depth));
            }
            InstructionKind::If {
                test,
                then,
                otherwise,
            } => {
                push_all(stack, otherwise.as_deref().unwrap_or_default(), depth);
                push_all(stack, then, depth);
                stack.push((Node::Input(test), depth));
            }
            InstructionKind::For { test, body } => {
                push_all(stack, body, depth);
                stack.push((Node::Input(test), depth));
            }
        },
    }
}

fn push_all<'p>(stack: &mut Vec<(Node<'p>, usize)>, list: &'p [Instruction], depth: usize) {
    stack.extend(list.iter().rev().map(|ins| (Node::Instruction(ins), depth)));
}

fn push_args<'p>(stack: &mut Vec<(Node<'p>, usize)>, args: &'p [(String, Input)], depth: usize) {
    stack.extend(args.iter().rev().map(|(_, arg)| (Node::Input(arg), depth)));
}

#[cfg(test)]
mod tests {
    use super::inside;

    #[test]
    fn a_part_is_inside_by_whole_path_segments() {
        assert!(inside("/do/1/set/a", "/do/1"));
        assert!(inside("/do/1/do/0/ret", "/do/1"));
        assert!(!inside("/do/1", "/do/1"));
        assert!(!inside("/do/10", "/do/1"));
        assert!(!inside("/do/2", "/do/1"));
        // The root holds everything in the body; `fn` bodies run in calls,
        // deeper, which stepping tells apart by depth.
        assert!(inside("/do/0", ""));
        assert!(!inside("", ""));
    }
}
