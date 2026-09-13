//! Serving, and stopping.
//!
//! # The order a shutdown happens in
//!
//! 1. A signal arrives (`SIGINT` or `SIGTERM`).
//! 2. The listener stops accepting and in-flight requests are given
//!    `[shutdown] grace` to finish.
//! 3. Only then is the lease released.
//! 4. Then the database is closed.
//!
//! Step 3 is after step 2 rather than before it, and the order is load-bearing.
//! Releasing the lease first lets another node take the writer role while this
//! one is still draining, and the takeover fences this writer — so every
//! in-flight write fails, which is precisely what the grace period was for.
//! `topology.md` puts it as: a takeover is an interruption, not a drain. Here
//! the drain comes first and the takeover is invited afterwards.
//!
//! Releasing at all is worth doing rather than letting the term expire: an
//! expiry costs the next node a wait of up to a term for no reason, and a
//! rolling restart pays that per node.

use crate::error::{Fault, Started};
use core::time::Duration;
use slate_kernel::{KvReadStore, KvStore};
use slate_server::{Head, Leadership};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// Serve until a signal, then drain, then resign.
pub(crate) async fn run<S: KvStore + KvReadStore>(
    head: Head<S>,
    listener: TcpListener,
    leadership: Arc<Leadership>,
    grace: Duration,
) -> Started<()> {
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let (stop, stopped) = oneshot::channel::<()>();

    // Spawned rather than awaited inline so the drain can be bounded: the
    // shutdown future is inside `serve_with_incoming_shutdown`, and a timeout
    // wrapped around the whole call would cut the *serving* short rather than
    // the draining.
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(head.into_service())
            .serve_with_incoming_shutdown(incoming, async {
                // A sender dropped without sending means the process is
                // unwinding; shutting down is the right response either way.
                let _ = stopped.await;
            })
            .await
    });

    let signal = wait_for_signal().await;
    println!("STOPPING {signal}");

    let _ = stop.send(());
    match tokio::time::timeout(grace, server).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(why))) => {
            return Err(Fault::new(format!(
                "the server stopped with an error: {why}"
            )));
        }
        Ok(Err(why)) => {
            return Err(Fault::new(format!(
                "the serving task did not finish: {why}"
            )));
        }
        Err(_) => {
            // Not an error exit: the requests that did not finish are gone
            // either way, and a non-zero exit here would make a normal
            // restart under load look like a crash to whatever supervises it.
            eprintln!(
                "slate-serverd: requests were still in flight after {grace:?}; stopping anyway"
            );
        }
    }

    leadership.resign().await;
    Ok(())
}

/// The name of whichever signal arrived first.
///
/// `SIGTERM` because that is what a supervisor sends, and `SIGINT` because
/// that is what a person sends. A node that handled only one of the two would
/// be graceful in exactly the situation nobody is watching or exactly the
/// situation everybody is.
#[cfg(unix)]
async fn wait_for_signal() -> &'static str {
    use tokio::signal::unix::{SignalKind, signal};
    // A failure to install a handler is not worth refusing to shut down over;
    // the branch simply never fires and the other signal still works.
    let mut terminate = signal(SignalKind::terminate()).ok();
    let mut interrupt = signal(SignalKind::interrupt()).ok();
    match (terminate.as_mut(), interrupt.as_mut()) {
        (Some(term), Some(int)) => {
            tokio::select! {
                _ = term.recv() => "SIGTERM",
                _ = int.recv() => "SIGINT",
            }
        }
        (Some(term), None) => {
            term.recv().await;
            "SIGTERM"
        }
        (None, Some(int)) => {
            int.recv().await;
            "SIGINT"
        }
        (None, None) => {
            core::future::pending::<()>().await;
            "never"
        }
    }
}

#[cfg(not(unix))]
async fn wait_for_signal() -> &'static str {
    let _ = tokio::signal::ctrl_c().await;
    "ctrl-c"
}
