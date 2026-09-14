//! Stopping cleanly, however the jukebox is told to stop: `SIGTERM` from systemd or launchd,
//! Ctrl-C in a terminal, Windows signing out or shutting down, or Quit in the tray. Each of
//! them runs the same steps, once.

use std::sync::mpsc;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use jukebox_server::App;
use tokio::sync::oneshot;

/// How long open requests — a guest's browser halfway through fetching cover art — get to
/// finish. Well inside what launchd (20 s) and a Windows sign-out wait before killing.
const DRAIN: Duration = Duration::from_secs(3);

pub struct Shutdown {
    once: Once,
    app: Arc<App>,
    stop_serving: Mutex<Option<oneshot::Sender<()>>>,
    served: Mutex<Option<mpsc::Receiver<()>>>,
}

/// Given to the server: completes when serving should stop, and is told when it has.
pub struct Serving {
    pub stop: oneshot::Receiver<()>,
    pub done: mpsc::Sender<()>,
}

impl Shutdown {
    pub fn new(app: Arc<App>) -> (Arc<Shutdown>, Serving) {
        let (stop_serving, stop) = oneshot::channel();
        let (done, served) = mpsc::channel();
        let shutdown = Arc::new(Shutdown {
            once: Once::new(),
            app,
            stop_serving: Mutex::new(Some(stop_serving)),
            served: Mutex::new(Some(served)),
        });
        (shutdown, Serving { stop, done })
    }

    /// Stops the jukebox: no new connections, the open ones closed or given a moment to
    /// finish, playback stopped and the database checkpointed. Returns once that's done; the
    /// caller then ends the process. A second call — two ways of stopping arriving together —
    /// waits for the first to finish. Blocks, so never call it on the async runtime's threads.
    pub fn stop(&self, reason: &str) {
        self.once.call_once(|| {
            tracing::info!("stopping: {reason}");
            if let Some(stop) = self.stop_serving.lock().unwrap().take() {
                let _ = stop.send(());
            }
            // The room goes quiet at once. Live updates hold their connections open, so
            // they're closed for serving to finish.
            self.app.engine().player().pause();
            self.app.hub().close_all();
            if let Some(served) = self.served.lock().unwrap().take() {
                if served.recv_timeout(DRAIN).is_err() {
                    tracing::info!("connections still open after {DRAIN:?}; closing anyway");
                }
            }
            // The database last, once the requests that might still write are done.
            self.app.close();
            tracing::info!("stopped cleanly");
        });
    }
}

/// `SIGTERM` and `SIGINT` (Ctrl-C, or Ctrl-Break on Windows), listened for from as early
/// as the runtime exists: one that arrives while the jukebox is still starting waits, and
/// stops it cleanly once it's up, instead of ending the process mid-start.
pub struct Signals {
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    /// Only reaches a jukebox run from a terminal; signing out is handled in session.rs.
    #[cfg(windows)]
    ctrl_c: Option<tokio::signal::windows::CtrlC>,
    #[cfg(windows)]
    ctrl_break: Option<tokio::signal::windows::CtrlBreak>,
}

impl Signals {
    /// Must be called within the runtime.
    pub fn listen() -> Signals {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            Signals {
                terminate: signal(SignalKind::terminate()).expect("listening for SIGTERM"),
                interrupt: signal(SignalKind::interrupt()).expect("listening for SIGINT"),
            }
        }
        #[cfg(windows)]
        Signals {
            ctrl_c: tokio::signal::windows::ctrl_c().ok(),
            ctrl_break: tokio::signal::windows::ctrl_break().ok(),
        }
    }

    #[cfg(unix)]
    async fn next(&mut self) -> &'static str {
        tokio::select! {
            _ = self.terminate.recv() => "SIGTERM",
            _ = self.interrupt.recv() => "interrupted",
        }
    }

    #[cfg(windows)]
    async fn next(&mut self) -> &'static str {
        let ctrl_c = async {
            match self.ctrl_c.as_mut() {
                Some(signal) => {
                    signal.recv().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        let ctrl_break = async {
            match self.ctrl_break.as_mut() {
                Some(signal) => {
                    signal.recv().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = ctrl_c => "interrupted",
            _ = ctrl_break => "interrupted",
        }
    }
}

/// Waits for a signal, then stops. A second signal while stopping ends the process at once.
pub async fn on_signals(mut signals: Signals, shutdown: Arc<Shutdown>) {
    let reason = signals.next().await;
    tokio::spawn(async move {
        signals.next().await;
        tracing::warn!("stopping at once");
        std::process::exit(1);
    });
    let stopped = tokio::task::spawn_blocking(move || shutdown.stop(reason)).await;
    std::process::exit(if stopped.is_ok() { 0 } else { 1 });
}
