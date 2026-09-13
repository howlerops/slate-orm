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
//!
//! # Nagle, which costs 165x on a streaming read
//!
//! `tonic::transport::Server` sets `TCP_NODELAY` on connections it accepts
//! itself — and its own documentation says the setting *"is ignored when using
//! this method"* for `serve_with_incoming`, which is what a server that binds
//! its own listener has to call. So this accepts Nagled sockets unless it says
//! otherwise, and it did not.
//!
//! Nagle holds a small write back waiting for an ACK of the previous one, and
//! a gRPC server stream is exactly that shape: a header message, then a batch
//! of rows. Measured at four clients over loopback, a ten-row query took
//! **44.01 ms** with Nagle on and **267 µs** with it off, with a unary `Get`
//! unmoved either way (102 vs 109 µs) as the control — the delay is in the
//! second small write, so a one-message call never sees it. The kernel's
//! delayed-ACK counter reads 1.16 per operation with it on and 0.00 with it
//! off, which is the mechanism rather than an inference from the timing.
//!
//! This also explains three things previously written down as unexplained: a
//! ~2.3 ms step in first-row latency around a batch of 125 rows, bimodal drain
//! times filed as "the machine", and 40 ms tails in the first concurrency run.
//! All three are the same socket.

use crate::error::{Fault, Started};
use core::time::Duration;
use slate_kernel::{KvReadStore, KvStore};
use slate_server::{Head, Leadership};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_stream::StreamExt as _;

/// Turn Nagle off on an accepted connection.
///
/// A failure is swallowed rather than dropping the connection: the socket
/// still works, it is only slow, and refusing to serve a client because an
/// optimisation could not be applied is the worse trade. Nothing in the
/// standard library makes this fail on a connected TCP socket in practice.
///
/// Separated from the accept loop so it can be tested against a real socket
/// by reading the flag back, rather than by timing a request and hoping the
/// margin holds on a loaded machine.
fn without_nagle(socket: TcpStream) -> TcpStream {
    let _ = socket.set_nodelay(true);
    socket
}

/// Serve until a signal, then drain, then resign.
pub(crate) async fn run<S: KvStore + KvReadStore>(
    head: Head<S>,
    listener: TcpListener,
    leadership: Arc<Leadership>,
    grace: Duration,
    concurrency: Option<usize>,
    request_timeout: Option<Duration>,
) -> Started<()> {
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener)
        .map(|accepted| accepted.map(without_nagle));
    let (stop, stopped) = oneshot::channel::<()>();

    // Spawned rather than awaited inline so the drain can be bounded: the
    // shutdown future is inside `serve_with_incoming_shutdown`, and a timeout
    // wrapped around the whole call would cut the *serving* short rather than
    // the draining.
    let server = tokio::spawn(async move {
        let mut builder = tonic::transport::Server::builder();
        // Applied to the builder rather than per handler: these bound the node
        // as a whole, and a caller's leverage here is the number of requests
        // they can have in flight, not the number per connection.
        //
        // Left unset by default. A concurrency limit low enough to protect a
        // small node is low enough to break a large one, and there is no
        // number that is right without knowing the machine — but until now
        // there was no way to say one at all, which is the actual defect.
        if let Some(limit) = concurrency {
            builder = builder.concurrency_limit_per_connection(limit);
        }
        if let Some(timeout) = request_timeout {
            builder = builder.timeout(timeout);
        }
        builder
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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::bool_assert_comparison
)]
mod tests {
    use super::without_nagle;
    use tokio::net::{TcpListener, TcpStream};

    /// The flag is read back off a real connected socket.
    ///
    /// Timing it would be the obvious test and the wrong one: the difference
    /// is 165x on a quiet machine and the assertion would still be a bet on
    /// how loaded the box is. The flag either got set or it did not.
    #[tokio::test]
    async fn an_accepted_connection_has_nagle_turned_off() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let connecting = tokio::spawn(async move { TcpStream::connect(address).await });
        let (accepted, _) = listener.accept().await.expect("accept");
        let _client = connecting.await.expect("join").expect("connect");

        // The control: an accepted socket does not come with it set, so the
        // assertion below is about `without_nagle` and not about a default.
        assert_eq!(
            accepted.nodelay().expect("read the flag"),
            false,
            "an accepted socket already had TCP_NODELAY, so this proves nothing"
        );
        let accepted = without_nagle(accepted);
        assert!(accepted.nodelay().expect("read the flag"));
    }
}
