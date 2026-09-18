//! The scrape endpoint: the counters over HTTP, on a port of their own.
//!
//! # Why a second listener rather than a gRPC method
//!
//! Adding `Metrics` to `records.proto` would need no new dependency, no second
//! port and no second thing to shut down. It was rejected because nothing that
//! scrapes metrics speaks gRPC: Prometheus, Grafana Agent, Vector and every
//! hosted equivalent fetch a URL, and an RPC would mean an adapter process per
//! deployment to turn one into the other. An endpoint nobody's monitoring can
//! reach is not monitoring.
//!
//! A separate port rather than a path on the gRPC one, because the gRPC port
//! is where the data is and this port is where the *shape* of the traffic is.
//! They want different firewall rules, and one address cannot have two.
//!
//! # Why hyper rather than forty lines of socket code
//!
//! A scrape is one `GET`, so answering it by hand looked tempting and is how
//! this was first sketched. It is a parser on a listening socket, which is the
//! category of code this repository fuzzes rather than writes on a hunch — and
//! hyper 1 is already in the tree under tonic, so "no new dependency" was not
//! actually on offer, only "no new *declared* dependency".
//!
//! # What it does not do
//!
//! No authentication, and the body is a list of every method this node serves
//! with call counts and latencies. That is a real disclosure — it is traffic
//! analysis handed over on request — which is why it is off unless configured,
//! why the default in every example is loopback, and why `main.rs` warns when
//! the address is not. Putting a token on it was considered and left out: a
//! shared secret in the same configuration file, checked by string comparison
//! on an unencrypted port, would be a thing that *looks* like authentication.
//! The honest answer is a firewall, and saying so.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::observe::Counters;

/// The exposition format's content type, version and all.
///
/// The version matters: a scraper reading `text/plain` with no `version`
/// parameter is entitled to guess, and some guess OpenMetrics, which requires
/// a trailing `# EOF` this does not write.
const EXPOSITION: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Serve `/metrics` until `stopped` resolves.
///
/// Takes a bound listener rather than an address for the reason the gRPC
/// listener does: binding before the banner means a scraper that reads the
/// banner and connects immediately finds a socket already accepting, and it is
/// what lets a test ask for port 0 and be told which port it got.
pub(crate) async fn serve(
    listener: TcpListener,
    counters: Arc<Counters>,
    mut stopped: oneshot::Receiver<()>,
) {
    loop {
        let accepted = tokio::select! {
            // Biased so a stop signal wins a race with a connection arriving
            // at the same moment. Without it a scraper polling every second
            // can keep the select busy through the whole shutdown, and the
            // node's last act is answering a scrape it did not have to.
            biased;
            _ = &mut stopped => return,
            accepted = listener.accept() => accepted,
        };
        let Ok((socket, _)) = accepted else {
            // An accept error here is a per-connection problem — the listener
            // is still bound. Dropping the node's metrics because one client
            // hung up mid-handshake would make the endpoint less reliable than
            // the thing it reports on.
            continue;
        };
        let counters = Arc::clone(&counters);
        // Per connection, so one slow scraper cannot hold up another and a
        // panic in one cannot take the listener down with it.
        tokio::spawn(async move {
            let served = http1::Builder::new().serve_connection(
                TokioIo::new(socket),
                service_fn(move |request| {
                    let counters = Arc::clone(&counters);
                    async move { Ok::<_, Infallible>(answer(&request, &counters)) }
                }),
            );
            // Dropped: a scraper that disconnects mid-response is ordinary and
            // is not this node's problem to report.
            let _ = served.await;
        });
    }
}

/// Answer one request.
///
/// Separated from the socket so it can be tested without one: the routing is
/// the part with decisions in it, and a test that has to bind a port to check
/// that `/` is a 404 is a test nobody writes.
///
/// Generic over the body rather than taking hyper's `Incoming`, which has no
/// public constructor — so a request carrying one can only come from a real
/// socket, and the routing could not be tested at all. Nothing here reads the
/// body; a scrape has none.
fn answer<B>(request: &Request<B>, counters: &Counters) -> Response<Full<Bytes>> {
    let body = |status: StatusCode, kind: &str, text: String| {
        Response::builder()
            .status(status)
            .header(hyper::header::CONTENT_TYPE, kind)
            .body(Full::new(Bytes::from(text)))
            // The builder fails only on a header this code does not build.
            // `unwrap_or_default` rather than `expect` so a monitoring
            // endpoint cannot be the thing that kills the node: an empty 200
            // is a worse answer than the right one and a better one than a
            // panic in a spawned task.
            .unwrap_or_default()
    };
    match (request.method(), request.uri().path()) {
        // `HEAD` as well as `GET`, because health checks send it and a 405
        // there reads as "the endpoint is broken".
        (&Method::GET | &Method::HEAD, "/metrics") => {
            body(StatusCode::OK, EXPOSITION, counters.prometheus())
        }
        // A bare `/` is what a person types to see whether this is up, so it
        // answers rather than 404ing, and says the one thing they need.
        (&Method::GET | &Method::HEAD, "/") => body(
            StatusCode::OK,
            "text/plain; charset=utf-8",
            "slate-serverd metrics: /metrics\n".to_owned(),
        ),
        (&Method::GET | &Method::HEAD, _) => body(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            "not found; this endpoint serves /metrics\n".to_owned(),
        ),
        _ => body(
            StatusCode::METHOD_NOT_ALLOWED,
            "text/plain; charset=utf-8",
            "this endpoint is read-only; use GET /metrics\n".to_owned(),
        ),
    }
}

/// Bind the metrics listener, or say why not.
///
/// A failure is the node's failure to start, not a warning: a deployment that
/// asked for metrics and silently got none is one whose dashboards are empty
/// for a reason nobody can see, and the usual cause — the port is taken — is
/// exactly the case where carrying on quietly is worst.
pub(crate) async fn bind(address: SocketAddr) -> Result<TcpListener, String> {
    TcpListener::bind(address)
        .await
        .map_err(|why| format!("`[observability] metrics_address = \"{address}\"`: {why}"))
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "a test that cannot fail loudly is a test with a second failure mode"
)]
mod tests {
    use super::*;
    use core::time::Duration;

    fn asked(method: Method, path: &str) -> Response<Full<Bytes>> {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .body(())
            .expect("a request this test wrote");
        answer(&request, &Counters::default())
    }

    #[test]
    fn a_scrape_answers_with_the_exposition_content_type() {
        // The `version=0.0.4` is load-bearing and not decoration: a body
        // served as bare `text/plain` may be read as OpenMetrics, which
        // requires a trailing `# EOF` this does not write, and the scrape then
        // fails on a format error rather than on anything being wrong.
        let answered = asked(Method::GET, "/metrics");
        assert_eq!(answered.status(), StatusCode::OK);
        let kind = answered.headers().get(hyper::header::CONTENT_TYPE);
        assert_eq!(kind.and_then(|v| v.to_str().ok()), Some(EXPOSITION));
    }

    #[test]
    fn a_path_that_is_not_metrics_is_a_404_and_says_where_to_go() {
        let answered = asked(Method::GET, "/varz");
        assert_eq!(answered.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn the_root_answers_rather_than_404s() {
        // What somebody types into a browser to find out whether the port is
        // the right one. A 404 there is technically correct and tells them
        // nothing.
        assert_eq!(asked(Method::GET, "/").status(), StatusCode::OK);
    }

    #[test]
    fn a_write_is_refused_rather_than_ignored() {
        // A 404 on a POST would suggest the path is wrong; a 200 would suggest
        // it worked. This endpoint has no writes and says so.
        for method in [Method::POST, Method::PUT, Method::DELETE] {
            assert_eq!(
                asked(method.clone(), "/metrics").status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{method}"
            );
        }
    }

    #[tokio::test]
    async fn a_real_scraper_gets_the_counters_over_a_real_socket() {
        // The unit tests above call `answer` directly, which cannot catch the
        // listener never being spawned, the wrong counters being handed to it,
        // or a body that never completes. This is the one that goes through a
        // socket.
        let counters = Arc::new(Counters::default());
        counters.record("/slate.v1.Records/Query", Duration::from_millis(3), false);

        let listener = bind("127.0.0.1:0".parse().expect("a literal address"))
            .await
            .expect("a free port");
        let address = listener.local_addr().expect("a bound listener");
        let (stop, stopped) = oneshot::channel();
        let serving = tokio::spawn(serve(listener, Arc::clone(&counters), stopped));

        let body = scrape(address, "/metrics").await;
        assert!(
            body.contains("slate_requests_total{method=\"/slate.v1.Records/Query\"} 1"),
            "{body}"
        );
        assert!(body.contains("slate_request_head_seconds_bucket"), "{body}");

        // The stop signal ends the accept loop rather than being noticed on
        // the next connection — asserted by joining, which hangs if it is not
        // true.
        stop.send(()).expect("the server is still listening");
        tokio::time::timeout(Duration::from_secs(5), serving)
            .await
            .expect("the metrics server should stop when told to")
            .expect("the metrics server should not panic");
    }

    /// One HTTP/1.1 request over a fresh connection, returning the body.
    ///
    /// Hand-written rather than pulling in an HTTP client for a test: the
    /// request is seventy bytes and the response is read to end-of-stream
    /// because the server closes the connection. Using hyper's client here
    /// would test hyper against hyper.
    async fn scrape(address: SocketAddr, path: &str) -> String {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let mut socket = tokio::net::TcpStream::connect(address)
            .await
            .expect("the metrics port should accept");
        socket
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: m\r\nConnection: close\r\n\r\n").as_bytes(),
            )
            .await
            .expect("a request should be writable");
        let mut answered = String::new();
        socket
            .read_to_string(&mut answered)
            .await
            .expect("the response should be readable");
        answered
    }
}
