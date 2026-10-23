use sofie_core::Result;

/// What a shell reports for a process ended by Ctrl-C.
const FORCED_EXIT: i32 = 130;

/// Listening starts before this returns, so a signal sent right after is not missed. A second
/// signal exits at once, so a request or job that never ends cannot hold the process up.
#[cfg(unix)]
pub(crate) fn stop_signal() -> Result<impl Future<Output = ()>> {
    let mut stops = Stops::listen()?;
    Ok(async move {
        stops.next().await;
        tokio::spawn(async move {
            stops.next().await;
            exit_forced();
        });
    })
}

#[cfg(unix)]
struct Stops {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Stops {
    fn listen() -> Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};

        let listen = |kind: SignalKind, name: &str| {
            signal(kind).map_err(|error| {
                sofie_core::Error::internal(format!("listening for {name}"), error)
            })
        };
        Ok(Self {
            interrupt: listen(SignalKind::interrupt(), "SIGINT")?,
            terminate: listen(SignalKind::terminate(), "SIGTERM")?,
        })
    }

    async fn next(&mut self) {
        std::future::poll_fn(|context| {
            if self.interrupt.poll_recv(context).is_ready() {
                return std::task::Poll::Ready(());
            }
            self.terminate.poll_recv(context).map(drop)
        })
        .await;
    }
}

#[cfg(not(unix))]
pub(crate) fn stop_signal() -> Result<impl Future<Output = ()>> {
    Ok(async {
        ctrl_c().await;
        tokio::spawn(async {
            ctrl_c().await;
            exit_forced();
        });
    })
}

#[cfg(not(unix))]
async fn ctrl_c() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::warn!(error.kind = "signal", error = %error, "listening for Ctrl-C failed");
    }
}

fn exit_forced() {
    tracing::warn!(
        error.kind = "signal",
        "a second stop signal arrived; exiting without waiting for open requests and jobs"
    );
    std::process::exit(FORCED_EXIT);
}

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::time::Duration;

    use super::Stops;

    /// `Stops` rather than `stop_signal`, whose second signal would end the test process.
    async fn resolved_by(signal: &str) {
        let mut stops = Stops::listen().unwrap();
        let stop = stops.next();

        let sent = Command::new("kill")
            .args([signal, &std::process::id().to_string()])
            .status()
            .unwrap();

        assert!(sent.success());
        tokio::time::timeout(Duration::from_secs(5), stop)
            .await
            .expect("the signal stops the process");
    }

    #[tokio::test]
    async fn sigterm_resolves_the_stop_signal() {
        resolved_by("-TERM").await;
    }

    /// Sent before the future is first polled, which would once end the process outright.
    #[tokio::test]
    async fn sigint_resolves_the_stop_signal_listened_for_before_it_is_polled() {
        resolved_by("-INT").await;
    }
}
