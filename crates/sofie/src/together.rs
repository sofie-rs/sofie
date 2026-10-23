//! The web server and the worker in one process: either half ending, at boot or later, stops
//! the other and the process, so it never serves without its jobs or runs jobs nobody serves.

use std::future::poll_fn;
use std::pin::pin;
use std::task::Poll;

use sofie_core::{Error, Result};
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub(crate) struct Together<S> {
    signal: S,
    stop: watch::Sender<bool>,
}

enum First {
    Signal,
    Web(Result<()>),
    Worker(Result<()>),
}

impl<S: Future<Output = ()>> Together<S> {
    pub(crate) fn new(signal: S) -> Self {
        Self {
            signal,
            stop: watch::channel(false).0,
        }
    }

    /// Completes once the signal arrives or the other half ends.
    pub(crate) fn stopped(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut stopped = self.stop.subscribe();
        async move {
            // An error means the sender is gone, which also means stop.
            let _ = stopped.wait_for(|stopped| *stopped).await;
        }
    }

    pub(crate) async fn run(
        self,
        web: impl Future<Output = Result<()>>,
        worker: JoinHandle<Result<()>>,
    ) -> Result<()> {
        let Self { signal, stop } = self;
        let (mut signal, mut web, mut worker) = (pin!(signal), pin!(web), worker);
        let first = poll_fn(|context| {
            if signal.as_mut().poll(context).is_ready() {
                return Poll::Ready(First::Signal);
            }
            if let Poll::Ready(served) = web.as_mut().poll(context) {
                return Poll::Ready(First::Web(served));
            }
            match pin!(&mut worker).poll(context) {
                Poll::Ready(finished) => Poll::Ready(First::Worker(joined(finished))),
                Poll::Pending => Poll::Pending,
            }
        })
        .await;
        stop.send_replace(true);
        match first {
            First::Signal => {
                let served = web.await;
                let jobs = joined(worker.await);
                if served.is_err() {
                    log_after(jobs);
                    return served;
                }
                jobs
            }
            First::Web(served) => {
                let error = ended_early("web", served);
                log_after(joined(worker.await));
                Err(error)
            }
            First::Worker(outcome) => {
                let error = ended_early("worker", outcome);
                log_after(web.await);
                Err(error)
            }
        }
    }
}

fn joined(worked: std::result::Result<Result<()>, tokio::task::JoinError>) -> Result<()> {
    worked.unwrap_or_else(|error| Err(Error::internal("running the worker", error)))
}

fn ended_early(half: &'static str, outcome: Result<()>) -> Error {
    let error = outcome.err().unwrap_or_else(|| {
        Error::internal_message(format!(
            "the {half} half stopped before the process was told to"
        ))
    });
    tracing::error!(
        error.kind = "serve",
        error = %error,
        half,
        "one half of the process stopped; stopping the other"
    );
    error
}

fn log_after(outcome: Result<()>) {
    if let Err(error) = outcome {
        tracing::error!(error.kind = "serve", error = %error, "stopping the other half failed");
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::time::Duration;

    use super::*;

    async fn within_a_second(run: impl Future<Output = Result<()>>) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(1), run)
            .await
            .expect("the process stopped")
    }

    #[tokio::test]
    async fn a_worker_failing_at_boot_stops_the_web_server_and_the_process() {
        let together = Together::new(pending());
        let web_stopped = together.stopped();
        let worker = tokio::spawn(async { Err(Error::internal_message("no schedules table")) });

        let outcome = within_a_second(together.run(
            async {
                web_stopped.await;
                Ok(())
            },
            worker,
        ))
        .await;

        assert!(
            matches!(&outcome, Err(Error::Internal { context, .. }) if context == "no schedules table"),
            "{outcome:?}"
        );
    }

    #[tokio::test]
    async fn a_panicking_worker_stops_the_web_server() {
        let together = Together::new(pending());
        let web_stopped = together.stopped();
        let worker = tokio::spawn(async { panic!("the worker crashed") });

        let outcome = within_a_second(together.run(
            async {
                web_stopped.await;
                Ok(())
            },
            worker,
        ))
        .await;

        assert!(
            matches!(outcome, Err(Error::Internal { .. })),
            "{outcome:?}"
        );
    }

    #[tokio::test]
    async fn a_web_server_that_ends_stops_the_worker() {
        let together = Together::new(pending());
        let worker = tokio::spawn({
            let stopped = together.stopped();
            async move {
                stopped.await;
                Ok(())
            }
        });

        let outcome = within_a_second(together.run(async { Ok(()) }, worker)).await;

        assert!(
            matches!(outcome, Err(Error::Internal { .. })),
            "{outcome:?}"
        );
    }

    #[tokio::test]
    async fn the_signal_stops_both_halves_cleanly() {
        let together = Together::new(async {});
        let web_stopped = together.stopped();
        let worker = tokio::spawn({
            let stopped = together.stopped();
            async move {
                stopped.await;
                Ok(())
            }
        });

        let outcome = within_a_second(together.run(
            async {
                web_stopped.await;
                Ok(())
            },
            worker,
        ))
        .await;

        assert!(outcome.is_ok(), "{outcome:?}");
    }

    #[tokio::test]
    async fn after_the_signal_a_worker_failing_beside_the_web_server_is_logged() {
        let telemetry = sofie_telemetry::capture();
        let together = Together::new(async {});
        let worker = tokio::spawn(async { Err(Error::internal_message("the queue was lost")) });

        let outcome = within_a_second(together.run(
            async { Err(Error::internal_message("the listener closed")) },
            worker,
        ))
        .await;

        assert!(outcome.is_err(), "{outcome:?}");
        assert!(telemetry.events().iter().any(|event| {
            event
                .fields
                .get("error")
                .is_some_and(|error| error.contains("the queue was lost"))
        }));
    }
}
