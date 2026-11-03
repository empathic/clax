//! The request an event was published because of: its id, the `cause`.
//!
//! The HTTP layer runs each request's handler inside [`scope`] with the
//! request's id; [`crate::EventBus::publish`] reads [`current`] and stamps
//! the event with it, so a subscriber can tell which request caused what it
//! sees. A future spawned from the handler (`tokio::spawn`) runs outside the
//! scope: its events carry no cause unless it is wrapped in [`scope`] again
//! with [`current`]'s value.
//!
//! No runtime is needed: the cause is set in a thread-local for the length
//! of each poll of the scoped future, and put back after, so it works on any
//! executor, the Worker's on wasm32 included.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

thread_local! {
    static CURRENT: RefCell<Option<Arc<str>>> = const { RefCell::new(None) };
}

/// The cause of the code running now: the id of the request whose
/// [`scope`] encloses it, if any.
pub fn current() -> Option<Arc<str>> {
    CURRENT.with(|c| c.borrow().clone())
}

/// Runs `f` with `cause` as [`current`] while it is polled.
pub fn scope<F: Future>(cause: Arc<str>, f: F) -> Scoped<F> {
    Scoped {
        cause,
        inner: Box::pin(f),
    }
}

/// Runs `f` synchronously with `cause` as [`current`].
pub fn with<R>(cause: Arc<str>, f: impl FnOnce() -> R) -> R {
    let _guard = Swap::enter(Some(cause));
    f()
}

/// A future run under a cause ([`scope`]).
pub struct Scoped<F> {
    cause: Arc<str>,
    inner: Pin<Box<F>>,
}

impl<F: Future> Future for Scoped<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let this = self.get_mut();
        let _guard = Swap::enter(Some(this.cause.clone()));
        this.inner.as_mut().poll(cx)
    }
}

/// Puts the previous cause back when dropped, a panic included.
struct Swap(Option<Arc<str>>);

impl Swap {
    fn enter(cause: Option<Arc<str>>) -> Self {
        Swap(CURRENT.with(|c| c.replace(cause)))
    }
}

impl Drop for Swap {
    fn drop(&mut self) {
        let prev = self.0.take();
        CURRENT.with(|c| *c.borrow_mut() = prev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_sets_and_restores() {
        assert_eq!(current(), None);
        with("a".into(), || {
            assert_eq!(current().as_deref(), Some("a"));
            with("b".into(), || assert_eq!(current().as_deref(), Some("b")));
            assert_eq!(current().as_deref(), Some("a"));
        });
        assert_eq!(current(), None);
    }

    #[tokio::test]
    async fn scope_holds_across_awaits_only_inside() {
        let seen = scope("r1".into(), async {
            tokio::task::yield_now().await;
            current()
        })
        .await;
        assert_eq!(seen.as_deref(), Some("r1"));
        assert_eq!(current(), None);
    }
}
