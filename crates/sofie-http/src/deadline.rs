use std::future::poll_fn;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use sofie_core::unpoisoned;
use tokio::time::Instant;

use crate::answer::Response;
use crate::caller::Caller;

pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A request's time limit, lifted once its commit starts: cutting the commit, or the
/// cookie that records it, would answer 503 for a change that was made. `None` once lifted.
#[derive(Clone)]
pub(crate) struct Deadline(Arc<Mutex<Option<Instant>>>);

/// While a file arrives, which the form's own time bounds instead.
#[cfg(feature = "uploads")]
pub(crate) struct Paused<'d>(&'d Deadline);

impl Default for Deadline {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Some(Instant::now() + REQUEST_TIMEOUT))))
    }
}

impl Deadline {
    pub(crate) fn lift(&self) {
        *unpoisoned::lock(&self.0) = None;
    }

    #[cfg(feature = "uploads")]
    pub(crate) fn paused(&self) -> Paused<'_> {
        self.lift();
        Paused(self)
    }

    fn ends(&self) -> Option<Instant> {
        *unpoisoned::lock(&self.0)
    }

    /// The deadline only moves later or lifts, so it is checked only when the timer fires.
    pub(crate) async fn within<F: Future>(&self, work: F) -> Option<F::Output> {
        let mut work = pin!(work);
        let mut timer = pin!(tokio::time::sleep(Duration::ZERO));
        poll_fn(|context| {
            if let Poll::Ready(output) = work.as_mut().poll(context) {
                return Poll::Ready(Some(output));
            }
            while timer.as_mut().poll(context).is_ready() {
                match self.ends() {
                    None => return Poll::Pending,
                    Some(ends) if ends > Instant::now() => timer.as_mut().reset(ends),
                    Some(_) => return Poll::Ready(None),
                }
            }
            Poll::Pending
        })
        .await
    }
}

/// For a page loaded after a commit lifted the request's deadline: cut, it still answers, so the
/// session cookie recording the commit is set.
#[cfg(feature = "engine")]
pub(crate) async fn limited_afresh(
    work: impl Future<Output = Response>,
    caller: Caller<'_>,
) -> Response {
    Deadline::default()
        .within(work)
        .await
        .unwrap_or_else(|| crate::pipeline::timed_out(caller))
}

#[cfg(feature = "uploads")]
impl Drop for Paused<'_> {
    fn drop(&mut self) {
        *unpoisoned::lock(&self.0.0) = Some(Instant::now() + REQUEST_TIMEOUT);
    }
}

#[cfg(test)]
mod tests {
    use tokio::time::sleep;

    use super::*;

    const LIMIT: Duration = REQUEST_TIMEOUT;

    #[tokio::test(start_paused = true)]
    async fn work_past_the_limit_is_cut_unless_it_lifted_the_deadline() {
        let slow = sleep(2 * LIMIT);
        assert_eq!(Deadline::default().within(slow).await, None);

        let deadline = Deadline::default();
        let committing = async {
            sleep(LIMIT / 2).await;
            deadline.lift();
            sleep(LIMIT).await;
            "committed"
        };
        assert_eq!(deadline.within(committing).await, Some("committed"));
    }

    #[cfg(feature = "engine")]
    #[tokio::test(start_paused = true)]
    async fn a_page_loaded_after_a_commit_has_a_limit_of_its_own() {
        let slow_page = async {
            sleep(2 * LIMIT).await;
            crate::answer::response(http::StatusCode::OK, None, "loaded")
        };

        let answered = limited_afresh(slow_page, Caller::Json).await;

        assert_eq!(answered.status(), http::StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test(start_paused = true)]
    async fn the_limit_starts_again_in_full_after_an_upload() {
        let deadline = Deadline::default();
        let uploading = async {
            sleep(LIMIT / 2).await;
            let paused = deadline.paused();
            sleep(3 * LIMIT).await;
            drop(paused);
            sleep(LIMIT * 3 / 4).await;
            "stored"
        };
        assert_eq!(deadline.within(uploading).await, Some("stored"));

        let deadline = Deadline::default();
        let slow_after = async {
            drop(deadline.paused());
            sleep(2 * LIMIT).await;
        };
        assert_eq!(deadline.within(slow_after).await, None);
    }
}
