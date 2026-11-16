//! The host side: [`Executor`]s, what they're called with, and [`Signal`].

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::{BoxError, Cause};
use crate::value::{Map, Value};

/// A boxed, `Send` future, as executors return.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What an executor produces: a JSON value (`Value::Null` for nothing), or an
/// error that fails the run with `executor_failed`.
pub type ExecutorResult = Result<Value, BoxError>;

/// What an executor is called with.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutorCall {
    /// The operation: what follows the executor id in the call.
    pub op: String,
    /// `arg` from the program, if any.
    pub arg: Option<Value>,
    /// The call's arguments, resolved, in program order (empty when it has none).
    pub input: Map,
    /// The run's abort signal, if one was given.
    pub signal: Option<Signal>,
}

/// Host code a program may call.
///
/// Most hosts don't implement this directly: [`EngineBuilder::executor`]
/// takes a plain closure and [`EngineBuilder::async_executor`] an async one.
/// Implement it for executors that keep state and want to borrow it while
/// they run.
///
/// Returning an error fails the run with `executor_failed`, unless the error
/// is itself a boxed [`crate::Error`], which fails it unchanged.
///
/// [`EngineBuilder::executor`]: crate::EngineBuilder::executor
/// [`EngineBuilder::async_executor`]: crate::EngineBuilder::async_executor
pub trait Executor: Send + Sync {
    /// Runs one call.
    fn call(&self, call: ExecutorCall) -> BoxFuture<'_, ExecutorResult>;
}

impl<E: Executor + ?Sized> Executor for Arc<E> {
    fn call(&self, call: ExecutorCall) -> BoxFuture<'_, ExecutorResult> {
        (**self).call(call)
    }
}

impl<E: Executor + ?Sized> Executor for Box<E> {
    fn call(&self, call: ExecutorCall) -> BoxFuture<'_, ExecutorResult> {
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
    fn call(&self, call: ExecutorCall) -> BoxFuture<'_, ExecutorResult> {
        Box::pin(std::future::ready((self.0)(call)))
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
/// let echo = executor::from_async_fn(|call| async move { Ok(call.arg.unwrap_or_default()) });
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
    fn call(&self, call: ExecutorCall) -> BoxFuture<'_, ExecutorResult> {
        Box::pin((self.0)(call))
    }
}

impl<F> fmt::Debug for AsyncFnExecutor<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AsyncFnExecutor")
    }
}

/// Cancels a run: the counterpart of JavaScript's `AbortController` and
/// `AbortSignal` in one cloneable handle.
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
