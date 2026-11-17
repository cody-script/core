//! [`Engine`] → [`CompiledProgram`] → [`RunResult`]: scopes, steps, errors.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use serde::{Deserialize, Serialize};

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
/// are `maxSteps` and `maxDepth`, as in the TypeScript package, and a missing
/// one takes its default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Limits {
    /// Nodes evaluated per run (each instruction, input, loop test and
    /// expression operand). Default 100 000.
    pub max_steps: u64,
    /// Maximum JSON nesting depth of a program, of an expression's
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

/// Options for one run.
#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    /// Initial variables, visible to the whole program.
    pub variables: Variables,
    /// Cancels the run between steps; passed on to executors.
    pub signal: Option<Signal>,
}

impl RunOptions {
    /// No variables, no signal.
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
}

impl From<Variables> for RunOptions {
    fn from(variables: Variables) -> Self {
        RunOptions {
            variables,
            signal: None,
        }
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

/// Whether `executors` provide `exe`'s function `op`.
fn provides(executors: &Executors, exe: &str, op: &str) -> bool {
    executors
        .get(exe)
        .is_some_and(|registered| registered.functions.contains(op))
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
            .is_some_and(|(exe, op)| provides(&self.inner.executors, exe, op))
    }

    /// Validates `program` (shape, depth, functions, expression syntax, and
    /// that the engine provides every external function it declares) once,
    /// and copies it: later changes to `program` can't reach the result.
    pub fn compile(&self, program: &Value) -> Result<CompiledProgram, Error> {
        let executors = &self.inner.executors;
        let root = validate(program, self.inner.limits.max_depth, &|exe, op| {
            provides(executors, exe, op)
        })?;
        Ok(CompiledProgram {
            engine: Arc::clone(&self.inner),
            program: program.clone(),
            root,
        })
    }

    /// `compile(program)?.run(options)`.
    pub async fn run(&self, program: &Value, options: RunOptions) -> Result<RunResult, Error> {
        self.compile(program)?.run(options).await
    }

    /// [`run`](Engine::run), blocking the current thread until it finishes.
    pub fn run_blocking(&self, program: &Value, options: RunOptions) -> Result<RunResult, Error> {
        block_on(self.run(program, options))
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
    program: Value,
    root: Program,
}

impl CompiledProgram {
    /// The program, as compiled (a copy of what was given to `compile`).
    pub fn program(&self) -> &Value {
        &self.program
    }

    /// Runs the program.
    pub async fn run(&self, options: RunOptions) -> Result<RunResult, Error> {
        let engine = &self.engine;
        run::run(&self.root, &engine.executors, engine.limits, options).await
    }

    /// [`run`](CompiledProgram::run), blocking the current thread until it
    /// finishes. Executors' futures are polled on this thread, so they must
    /// not need an async runtime's reactor (a Tokio timer, say); use `run`
    /// from within that runtime for those.
    pub fn run_blocking(&self, options: RunOptions) -> Result<RunResult, Error> {
        block_on(self.run(options))
    }
}

impl fmt::Debug for CompiledProgram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledProgram")
            .field("program", &self.program)
            .finish()
    }
}

/// Wakes the thread that's blocked in [`block_on`].
struct ThreadWaker(std::thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` to completion on this thread, parking between polls.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
        std::thread::park();
    }
}
