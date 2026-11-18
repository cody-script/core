//! [`Engine`] → [`CompiledProgram`] → [`RunResult`]: scopes, steps, errors.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::future::Future;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::block_on::block_on;
use crate::config::RunConfig;
use crate::debug::{self, Breakpoint, Debugger, Step};
use crate::error::Error;
use crate::executor::{Executor, ExecutorCall, ExecutorResult, Signal, from_async_fn, from_fn};
use crate::program::Program;
use crate::run;
use crate::validate::validate;
use crate::value::{Map, Value};

/// Variables by name, in the order they were first set.
pub type Variables = Map;

/// Bounds on what one program may consume.
///
/// Serializable, so limits can live in configuration; as data their fields
/// are `max_steps` and `max_depth`, and a missing one takes its default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    /// Nodes evaluated per run (each instruction, input, loop test and
    /// expression operand). Default 100 000.
    pub max_steps: u64,
    /// Maximum nesting depth of a program, of an expression's
    /// parentheses and unary operators, and of function calls while running.
    /// Default 128. Compiling recurses this deep, so keep it modest.
    pub max_depth: usize,
}

impl Limits {
    /// The default `max_steps`.
    pub const DEFAULT_MAX_STEPS: u64 = 100_000;
    /// The default `max_depth`.
    pub const DEFAULT_MAX_DEPTH: usize = 128;
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_steps: Self::DEFAULT_MAX_STEPS,
            max_depth: Self::DEFAULT_MAX_DEPTH,
        }
    }
}

/// Options for one run: its variables, a signal to cancel it, and a
/// debugger to pause it.
///
/// `'d` is how long the debugger lives, if there is one; it may borrow (a
/// closure that records its pauses in a local, say). Without one, it's any
/// lifetime at all.
#[derive(Default)]
pub struct RunOptions<'d> {
    /// Initial variables, visible to the whole program.
    pub variables: Variables,
    /// Cancels the run between steps; passed on to executors.
    pub signal: Option<Signal>,
    /// Where the debugger pauses the run, to begin with.
    pub(crate) breakpoints: Vec<Breakpoint>,
    /// The debugger, in a debugged run.
    pub(crate) debugger: Option<Box<dyn Debugger + 'd>>,
}

impl<'d> RunOptions<'d> {
    /// No variables, no signal, no debugger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the initial variables.
    pub fn with_variables(mut self, variables: Variables) -> Self {
        self.variables = variables;
        self
    }

    /// Sets the signal that cancels the run.
    pub fn with_signal(mut self, signal: Signal) -> Self {
        self.signal = Some(signal);
        self
    }

    /// Debugs the run: `debugger` pauses it before each step `breakpoints`
    /// name, and wherever its [`Action`](debug::Action)s say.
    ///
    /// Each time, the run waits for `debugger` to say how to go on, and may
    /// be given different breakpoints. With none, it never pauses. A
    /// breakpoint that names no step fails the run with `invalid_config` —
    /// before it starts, or when it goes on. Apart from the pauses it runs
    /// as it would without a debugger, with the same result, steps and
    /// limits — and fails with `aborted` if told to
    /// [`Stop`](debug::Action::Stop). See the [`debug`] module; the
    /// variables and breakpoints usually come from a
    /// [`RunConfig`](crate::RunConfig).
    ///
    /// A run that isn't debugged pays nothing for this: its loop is built
    /// without the checks.
    pub fn with_debugger(
        mut self,
        breakpoints: impl IntoIterator<Item = Breakpoint>,
        debugger: impl Debugger + 'd,
    ) -> Self {
        self.breakpoints = breakpoints.into_iter().collect();
        self.debugger = Some(Box::new(debugger));
        self
    }
}

impl From<Variables> for RunOptions<'_> {
    fn from(variables: Variables) -> Self {
        RunOptions::new().with_variables(variables)
    }
}

impl fmt::Debug for RunOptions<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("RunOptions");
        debug
            .field("variables", &self.variables)
            .field("signal", &self.signal);
        if self.debugger.is_some() {
            debug.field("breakpoints", &self.breakpoints);
        }
        debug.finish_non_exhaustive()
    }
}

/// What a run produced. Serializes as `{ value, variables, steps }`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunResult {
    /// What the program returned (`null` if it never `ret`urned).
    pub value: Value,
    /// The top-level variables when the program finished.
    pub variables: Variables,
    /// Steps consumed, for tuning `max_steps`.
    pub steps: u64,
}

/// The engine's executors by id, each with the functions it provides.
pub(crate) type Executors = HashMap<String, Registered>;

pub(crate) struct Registered {
    /// Its functions' names within it (`trim` for `text.trim`).
    pub functions: BTreeSet<String>,
    pub executor: Arc<dyn Executor>,
}

/// The executor that provides `exe`'s function `op`, if `executors` do.
pub(crate) fn provider<'e>(
    executors: &'e Executors,
    exe: &str,
    op: &str,
) -> Option<&'e Arc<dyn Executor>> {
    executors
        .get(exe)
        .filter(|registered| registered.functions.contains(op))
        .map(|registered| &registered.executor)
}

/// Every function `executors` provide, by full name (`text.trim`), sorted.
fn function_names(executors: &Executors) -> Vec<String> {
    let mut names: Vec<String> = executors
        .iter()
        .flat_map(|(id, r)| r.functions.iter().map(move |op| format!("{id}.{op}")))
        .collect();
    names.sort_unstable();
    names
}

struct Inner {
    executors: Executors,
    limits: Limits,
}

/// Compiles and runs programs. Each engine owns its executors; there is no
/// global registry, so two engines (two tenants, two tests) never see each
/// other's. Cloning is cheap and shares the executors.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

impl Engine {
    /// An engine with no executors and default limits: it runs pure programs.
    pub fn new() -> Self {
        Engine::builder().build()
    }

    /// Starts configuring an engine.
    pub fn builder() -> EngineBuilder {
        EngineBuilder::default()
    }

    /// The engine's limits.
    pub fn limits(&self) -> Limits {
        self.inner.limits
    }

    /// Whether the engine provides external function `name`
    /// (`"text.trim"`): its executor's id, a dot, and one of the functions
    /// it was registered with.
    pub fn has_function(&self, name: &str) -> bool {
        name.split_once('.')
            .is_some_and(|(exe, op)| provider(&self.inner.executors, exe, op).is_some())
    }

    /// Validates `program` (shape, depth, functions, expression syntax, and
    /// that the engine provides every external function it declares) once,
    /// and builds what runs it. The result keeps nothing of `program`, which
    /// may change or go once this returns.
    pub fn compile(&self, program: &Value) -> Result<CompiledProgram, Error> {
        Ok(CompiledProgram {
            engine: Arc::clone(&self.inner),
            root: self.tree(program)?,
        })
    }

    /// Compiles and runs `program` once. To run a program more than once,
    /// [`compile`](Engine::compile) it once instead.
    pub async fn run(&self, program: &Value, options: RunOptions<'_>) -> Result<RunResult, Error> {
        let root = self.tree(program)?;
        run::run(&root, self.inner.limits, options).await
    }

    /// [`run`](Engine::run), blocking the current thread until it finishes.
    pub fn run_blocking(
        &self,
        program: &Value,
        options: RunOptions<'_>,
    ) -> Result<RunResult, Error> {
        block_on(self.run(program, options))
    }

    /// Validates `program`, building the tree the engine runs.
    fn tree(&self, program: &Value) -> Result<Program, Error> {
        validate(program, self.inner.limits.max_depth, &self.inner.executors)
    }
}

impl fmt::Debug for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("functions", &function_names(&self.inner.executors))
            .field("limits", &self.inner.limits)
            .finish()
    }
}

/// Configures an [`Engine`].
#[derive(Default)]
pub struct EngineBuilder {
    executors: Executors,
    limits: Limits,
}

impl EngineBuilder {
    /// Provides the external functions `<id>.<function>`, one for each of
    /// `functions`, handled by `f`, which returns its result directly;
    /// `call.op` says which function was called.
    ///
    /// `.executor("text", ["trim", "empty"], f)` provides `text.trim` and
    /// `text.empty`, which a program may then declare in its `ext` (as
    /// `{ "text": ["trim", "empty"] }`) and call. Executor ids can't contain a
    /// dot, since a function's name splits at its first one; function names
    /// can (`num.round.up` is executor `num`'s function `round.up`).
    /// Registering an id again replaces the earlier executor.
    pub fn executor<F>(
        self,
        id: impl Into<String>,
        functions: impl IntoIterator<Item = impl Into<String>>,
        f: F,
    ) -> Self
    where
        F: Fn(ExecutorCall) -> ExecutorResult + Send + Sync + 'static,
    {
        self.custom_executor(id, functions, from_fn(f))
    }

    /// Provides the external functions `<id>.<function>`, handled by `f`,
    /// which returns a future the run awaits. See
    /// [`executor`](EngineBuilder::executor).
    pub fn async_executor<F, Fut>(
        self,
        id: impl Into<String>,
        functions: impl IntoIterator<Item = impl Into<String>>,
        f: F,
    ) -> Self
    where
        F: Fn(ExecutorCall) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ExecutorResult> + Send + 'static,
    {
        self.custom_executor(id, functions, from_async_fn(f))
    }

    /// Provides the external functions `<id>.<function>`, handled by any
    /// [`Executor`] (an `Arc<dyn Executor>` too). See
    /// [`executor`](EngineBuilder::executor).
    pub fn custom_executor(
        mut self,
        id: impl Into<String>,
        functions: impl IntoIterator<Item = impl Into<String>>,
        executor: impl Executor + 'static,
    ) -> Self {
        let registered = Registered {
            functions: functions.into_iter().map(Into::into).collect(),
            executor: Arc::new(executor),
        };
        self.executors.insert(id.into(), registered);
        self
    }

    /// Sets both limits.
    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Sets [`Limits::max_steps`].
    pub fn max_steps(mut self, max_steps: u64) -> Self {
        self.limits.max_steps = max_steps;
        self
    }

    /// Sets [`Limits::max_depth`].
    pub fn max_depth(mut self, max_depth: usize) -> Self {
        self.limits.max_depth = max_depth;
        self
    }

    /// The engine.
    pub fn build(self) -> Engine {
        Engine {
            inner: Arc::new(Inner {
                executors: self.executors,
                limits: self.limits,
            }),
        }
    }
}

impl fmt::Debug for EngineBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineBuilder")
            .field("functions", &function_names(&self.executors))
            .field("limits", &self.limits)
            .finish()
    }
}

/// A validated program, ready to run any number of times, concurrently if
/// you like: each run has its own state.
#[derive(Clone)]
pub struct CompiledProgram {
    engine: Arc<Inner>,
    root: Program,
}

impl CompiledProgram {
    /// Runs the program — under a debugger, if `options` has one (see
    /// [`RunOptions::with_debugger`]).
    pub async fn run(&self, options: RunOptions<'_>) -> Result<RunResult, Error> {
        run::run(&self.root, self.engine.limits, options).await
    }

    /// Checks a run configuration against the program: each of its
    /// breakpoints must name a step. Fails with `invalid_config`, listing
    /// every one that doesn't, as a run debugged at them would.
    pub fn check(&self, config: &RunConfig) -> Result<(), Error> {
        debug::check(&self.steps(), &config.breakpoints)
    }

    /// Every step of the program — each node a run can pause before, and a
    /// breakpoint can name — in the order written: its body's, then each
    /// function's in `fn`.
    pub fn steps(&self) -> Vec<Step<'_>> {
        debug::steps(&self.root)
    }

    /// [`run`](CompiledProgram::run), blocking the current thread until it
    /// finishes. Executors' futures are polled on this thread, so they must
    /// not need an async runtime's reactor (a Tokio timer, say); use `run`
    /// from within that runtime for those.
    pub fn run_blocking(&self, options: RunOptions<'_>) -> Result<RunResult, Error> {
        block_on(self.run(options))
    }
}

impl fmt::Debug for CompiledProgram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledProgram")
            .field("tree", &self.root)
            .finish()
    }
}
