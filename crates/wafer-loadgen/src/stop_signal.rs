//! SIGTERM and SIGINT handling shared by the publisher and subscriber.
//!
//! The first signal asks the run to stop and flush its artifacts. A second one
//! exits at once, for a run stuck somewhere that does not watch for the first.

use tokio::sync::watch;
use tracing::error;

pub struct StopSignal {
    reason: watch::Receiver<Option<&'static str>>,
}

impl StopSignal {
    /// Install the handlers. A signal that arrives after this returns is
    /// observed by [`Self::recv`] instead of killing the process.
    ///
    /// # Errors
    /// The OS refused to install a signal handler.
    pub fn install() -> std::io::Result<Self> {
        let mut signals = Signals::install()?;
        let (tx, reason) = watch::channel(None);
        tokio::spawn(async move {
            let (first, _) = signals.recv().await;
            tx.send_replace(Some(first));
            let (second, exit_code) = signals.recv().await;
            error!(signal = second, "Second stop signal received; exiting without flushing");
            std::process::exit(exit_code);
        });
        Ok(Self { reason })
    }

    /// Wait for the first stop signal and return its exit reason. Cancel-safe.
    pub async fn recv(&mut self) -> &'static str {
        let reason = self.reason.wait_for(Option::is_some).await.map(|reason| *reason);
        match reason {
            Ok(Some(reason)) => reason,
            _ => std::future::pending().await,
        }
    }
}

struct Signals {
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
}

impl Signals {
    #[cfg(unix)]
    fn install() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            terminate: signal(SignalKind::terminate())?,
            interrupt: signal(SignalKind::interrupt())?,
        })
    }

    #[cfg(not(unix))]
    const fn install() -> std::io::Result<Self> {
        Ok(Self {})
    }

    #[cfg(unix)]
    async fn recv(&mut self) -> (&'static str, i32) {
        tokio::select! {
            _ = self.terminate.recv() => ("sigterm", 143),
            _ = self.interrupt.recv() => ("sigint", 130),
        }
    }

    #[cfg(not(unix))]
    async fn recv(&mut self) -> (&'static str, i32) {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
        ("sigint", 130)
    }
}
