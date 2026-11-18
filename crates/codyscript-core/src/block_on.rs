//! [`block_on`]: runs a future to completion on the current thread, with
//! std alone — what the `_blocking` methods use, so the crate needs no async
//! runtime.

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

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

thread_local! {
    /// This thread's waker, made once and shared by every `block_on` on it.
    static WAKER: Waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
}

/// Polls `future` to completion on this thread, parking between polls.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    WAKER.with(|waker| {
        let mut cx = Context::from_waker(waker);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
                return output;
            }
            std::thread::park();
        }
    })
}
