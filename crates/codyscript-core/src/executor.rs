//! The host side: [`Executor`]s, which provide a program's external
//! functions, what they're called with, and [`Signal`].

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use crate::error::{BoxError, Cause};
use crate::value::{Map, Value};

/// A boxed, `Send` future, as an async executor's or a debugger's.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What an executor produces: a value (`Value::Null` for nothing), or an
/// error that fails the run with `function_failed`.
pub type ExecutorResult = Result<Value, BoxError>;

/// What an executor is called with: one call of one of its functions.
///
/// The function's name and `arg` are shared with the compiled program, so a
/// call copies neither; read them with [`op`](ExecutorCall::op) and
/// [`arg`](ExecutorCall::arg). The arguments, `input`, are the call's own.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutorCall {
    pub(crate) op: Arc<str>,
    pub(crate) arg: Option<Arc<Value>>,
    /// The call's arguments, resolved, in program order (empty when it has none).
    pub input: Map,
    pub(crate) signal: Option<Signal>,
    pub(crate) tag: Option<Arc<str>>,
}

impl ExecutorCall {
    /// A call of function `op` with `input`, and no `arg` or signal: what
    /// a host needs to test an executor. (Runs make their own.)
    pub fn new(op: impl Into<Arc<str>>, input: Map) -> Self {
        ExecutorCall {
            op: op.into(),
            arg: None,
            input,
            signal: None,
            tag: None,
        }
    }

    /// Sets the call's `arg`.
    pub fn with_arg(mut self, arg: impl Into<Arc<Value>>) -> Self {
        self.arg = Some(arg.into());
        self
    }

    /// Sets the run's signal.
    pub fn with_signal(mut self, signal: Signal) -> Self {
        self.signal = Some(signal);
        self
    }

    /// Sets the calling node's tag.
    pub fn with_tag(mut self, tag: impl Into<Arc<str>>) -> Self {
        self.tag = Some(tag.into());
        self
    }

    /// Which of the executor's functions: its name after the executor id
    /// and the first dot (`trim` for `text.trim`, `round.up` for
    /// `num.round.up`). Always one the executor was registered with.
    pub fn op(&self) -> &str {
        &self.op
    }

    /// `arg` from the program, if any.
    pub fn arg(&self) -> Option<&Value> {
        self.arg.as_deref()
    }

    /// The run's abort signal, if one was given.
    pub fn signal(&self) -> Option<&Signal> {
        self.signal.as_ref()
    }

    /// The `tag` of the `call` node making this call, if it has one: which
    /// call it is, for a host that traces or reports its functions' work
    /// (an editor's node id, say). Shared with the program, not copied.
    pub fn tag(&self) -> Option<&str> {
        self.tag.as_deref()
    }
}

/// What an executor returns: its result at once, or a future of it.
///
/// A synchronous executor's result needs no allocation
/// ([`ready`](ExecutorFuture::ready), or `.into()` from an
/// [`ExecutorResult`]); an asynchronous one is boxed
/// ([`new`](ExecutorFuture::new)).
pub struct ExecutorFuture<'a>(Pending<'a>);

enum Pending<'a> {
    /// The result, until it's taken.
    Ready(Option<ExecutorResult>),
    Future(BoxFuture<'a, ExecutorResult>),
}

impl<'a> ExecutorFuture<'a> {
    /// A result there already.
    pub fn ready(result: ExecutorResult) -> Self {
        ExecutorFuture(Pending::Ready(Some(result)))
    }

    /// A result still to come: `future`'s, boxed.
    pub fn new(future: impl Future<Output = ExecutorResult> + Send + 'a) -> Self {
        ExecutorFuture(Pending::Future(Box::pin(future)))
    }
}

impl From<ExecutorResult> for ExecutorFuture<'_> {
    fn from(result: ExecutorResult) -> Self {
        ExecutorFuture::ready(result)
    }
}

impl Future for ExecutorFuture<'_> {
    type Output = ExecutorResult;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<ExecutorResult> {
        match &mut self.0 {
            Pending::Ready(result) => {
                Poll::Ready(result.take().expect("an executor's result, polled once"))
            }
            Pending::Future(future) => future.as_mut().poll(cx),
        }
    }
}

impl fmt::Debug for ExecutorFuture<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            Pending::Ready(_) => "ExecutorFuture(ready)",
            Pending::Future(_) => "ExecutorFuture(pending)",
        })
    }
}

/// Host code that provides external functions: registered under an id
/// (`text`) with the functions it provides (`trim`, `empty`), which programs
/// declare in `ext` and call as `text.trim`, `text.empty`.
///
/// Most hosts don't implement this directly: [`EngineBuilder::executor`]
/// takes a plain closure and [`EngineBuilder::async_executor`] an async one.
/// Implement it for executors that keep state and want to borrow it while
/// they run.
///
/// Returning an error fails the run with `function_failed`, unless the error
/// is itself a boxed [`crate::Error`], which fails it unchanged.
///
/// [`EngineBuilder::executor`]: crate::EngineBuilder::executor
/// [`EngineBuilder::async_executor`]: crate::EngineBuilder::async_executor
pub trait Executor: Send + Sync {
    /// Runs one call: returns its result at once
    /// ([`ExecutorFuture::ready`]), or a future of it
    /// ([`ExecutorFuture::new`]).
    fn call(&self, call: ExecutorCall) -> ExecutorFuture<'_>;
}

impl<E: Executor + ?Sized> Executor for Arc<E> {
    fn call(&self, call: ExecutorCall) -> ExecutorFuture<'_> {
        (**self).call(call)
    }
}

impl<E: Executor + ?Sized> Executor for Box<E> {
    fn call(&self, call: ExecutorCall) -> ExecutorFuture<'_> {
        (**self).call(call)
    }
}

/// An [`Executor`] made from a synchronous closure; see [`from_fn`].
#[derive(Clone)]
pub struct FnExecutor<F>(F);

/// An executor from a closure that returns its result directly.
///
/// ```
/// use codyscript_core::{executor, Executor};
/// let upper = executor::from_fn(|call| {
///     let text = call.input.get("value").and_then(|v| v.as_str()).unwrap_or("");
///     Ok(text.to_uppercase().into())
/// });
/// # let _: &dyn Executor = &upper;
/// ```
pub fn from_fn<F>(f: F) -> FnExecutor<F>
where
    F: Fn(ExecutorCall) -> ExecutorResult + Send + Sync,
{
    FnExecutor(f)
}

impl<F> Executor for FnExecutor<F>
where
    F: Fn(ExecutorCall) -> ExecutorResult + Send + Sync,
{
    fn call(&self, call: ExecutorCall) -> ExecutorFuture<'_> {
        ExecutorFuture::ready((self.0)(call))
    }
}

impl<F> fmt::Debug for FnExecutor<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FnExecutor")
    }
}

/// An [`Executor`] made from a closure returning a future; see [`from_async_fn`].
#[derive(Clone)]
pub struct AsyncFnExecutor<F>(F);

/// An executor from a closure that returns a future, awaited by the run.
///
/// ```
/// use codyscript_core::{executor, Executor};
/// let echo = executor::from_async_fn(|call| async move { Ok(call.arg().cloned().unwrap_or_default()) });
/// # let _: &dyn Executor = &echo;
/// ```
pub fn from_async_fn<F, Fut>(f: F) -> AsyncFnExecutor<F>
where
    F: Fn(ExecutorCall) -> Fut + Send + Sync,
    Fut: Future<Output = ExecutorResult> + Send + 'static,
{
    AsyncFnExecutor(f)
}

impl<F, Fut> Executor for AsyncFnExecutor<F>
where
    F: Fn(ExecutorCall) -> Fut + Send + Sync,
    Fut: Future<Output = ExecutorResult> + Send + 'static,
{
    fn call(&self, call: ExecutorCall) -> ExecutorFuture<'_> {
        ExecutorFuture::new((self.0)(call))
    }
}

impl<F> fmt::Debug for AsyncFnExecutor<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AsyncFnExecutor")
    }
}

/// Cancels a run: one cloneable handle that both aborts and is checked (like
/// an `AbortController` and its `AbortSignal` together).
///
/// Give a clone to [`RunOptions::with_signal`](crate::RunOptions::with_signal)
/// and keep one; [`abort`](Signal::abort) from anywhere (another thread, an
/// executor) stops the run at its next step with `aborted`. Executors receive
/// the signal too, to stop their own work. Clones compare equal.
#[derive(Clone, Default)]
pub struct Signal {
    inner: Arc<SignalInner>,
}

#[derive(Default)]
struct SignalInner {
    aborted: AtomicBool,
    reason: Mutex<Option<Cause>>,
}

impl Signal {
    /// A signal that hasn't been aborted.
    pub fn new() -> Self {
        Self::default()
    }

    /// Aborts, with no reason. Only the first abort counts.
    pub fn abort(&self) {
        self.set(None);
    }

    /// Aborts; `reason` becomes the `aborted` error's cause. Only the first
    /// abort counts.
    pub fn abort_with(&self, reason: impl Into<BoxError>) {
        self.set(Some(Cause::from(reason.into())));
    }

    fn set(&self, reason: Option<Cause>) {
        let mut slot = self.inner.reason.lock().unwrap_or_else(|e| e.into_inner());
        if !self.inner.aborted.load(Ordering::Acquire) {
            *slot = reason;
            self.inner.aborted.store(true, Ordering::Release);
        }
    }

    /// Whether [`abort`](Signal::abort) has been called.
    pub fn is_aborted(&self) -> bool {
        self.inner.aborted.load(Ordering::Acquire)
    }

    /// Why it was aborted, if a reason was given.
    pub fn reason(&self) -> Option<Cause> {
        self.inner
            .reason
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl PartialEq for Signal {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for Signal {}

impl fmt::Debug for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Signal")
            .field("aborted", &self.is_aborted())
            .field("reason", &self.reason().map(|r| r.to_string()))
            .finish()
    }
}
