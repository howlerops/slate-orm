//! A node-wide cap on **open response streams**, not on admitted requests.
//!
//! `[limits] max_concurrent_requests` became
//! `tower::limit::GlobalConcurrencyLimitLayer` and bounds the right thing for
//! what it is: one semaphore for the process, taken when a request is admitted
//! and released when the response *future* resolves. For a server-streaming
//! RPC that is **before any row is read** — the handler returns a stream and
//! the permit goes back immediately — so a caller can hold any number of open
//! reads under a limit of one.
//!
//! That is measured, not argued: `ledger/2026-09-29-ten-streams-under-a-limit-
//! of-one.md` opened ten concurrent reads against a node configured for one
//! and all ten were served, and recorded the gap as
//!
//! > **Nothing bounds concurrent streams instead.** This measures the absence
//! > and adds no mechanism. A cap on open response streams is a different
//! > layer — tonic's `max_concurrent_streams` on the HTTP/2 connection is the
//! > obvious candidate and is per-connection, which is the scope mistake this
//! > setting was just fixed for. Whoever adds it should read that entry first.
//!
//! This is that layer, and it is not tonic's. The permit is acquired before
//! the inner service is called and **moved into the response body**, so it is
//! returned when the body is dropped — which for a streaming read is when the
//! last row has been sent or the caller has gone away. One semaphore, in an
//! `Arc`, shared by every connection: the scope mistake the entry warns about
//! is the whole reason this is here rather than a builder setting.
//!
//! # Refused, not queued
//!
//! A caller over the cap gets `RESOURCE_EXHAUSTED` immediately. The admission
//! limiter queues, and queueing is right there: a request waiting to start is
//! a request that will finish. A *stream* is held by the caller, for as long
//! as the caller likes, so a queue in front of one is a queue behind something
//! with no deadline — and a client that opens `n + 1` streams and reads them
//! round-robin would wait for a permit that only it can release. Refusing
//! turns a deadlock into an error the caller can retry, and makes the bound
//! observable in the one way a deployment needs: the refusal is counted like
//! any other failure.
//!
//! # What it still does not bound
//!
//! Bytes, and time. A single stream reading a hundred million rows takes one
//! permit, and this says nothing about how long it may hold it.

use core::task::{Context, Poll};
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};

/// The gRPC status a caller over the cap sees.
///
/// `RESOURCE_EXHAUSTED` rather than `UNAVAILABLE`, which is the other
/// candidate: `UNAVAILABLE` tells a client to retry the same call elsewhere or
/// later and is what a node says when it is *down*, and every client here
/// retries it. This node is up and the caller is over a quota, so the honest
/// code is the one whose name says so — and it is the code the execution
/// ceilings already use for the same reason.
const REFUSED: &str = "8";

/// Layer form, so it composes with the rest of the builder's stack.
#[derive(Clone)]
pub(crate) struct StreamLimitLayer {
    permits: Arc<Semaphore>,
}

impl StreamLimitLayer {
    /// A cap of `open` streams across the node.
    pub(crate) fn new(open: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(open)),
        }
    }
}

impl<S> tower::Layer<S> for StreamLimitLayer {
    type Service = StreamLimit<S>;

    fn layer(&self, inner: S) -> Self::Service {
        StreamLimit {
            inner,
            // `Arc::clone`, not a new semaphore. The layer is cloned per
            // connection, and a per-clone semaphore is exactly the
            // per-connection bound this exists to avoid: it is the one-word
            // difference that made `concurrency_limit_per_connection` read as
            // correct for weeks.
            permits: Arc::clone(&self.permits),
        }
    }
}

/// The service the layer makes.
#[derive(Clone)]
pub(crate) struct StreamLimit<S> {
    inner: S,
    permits: Arc<Semaphore>,
}

impl<S, B> tower::Service<http::Request<B>> for StreamLimit<S>
where
    S: tower::Service<http::Request<B>, Response = http::Response<tonic::body::Body>>,
    S::Future: Send + 'static,
    S::Error: 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = std::pin::Pin<
        Box<dyn core::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, request: http::Request<B>) -> Self::Future {
        // `try_acquire_owned`, in `call` rather than in `poll_ready`.
        //
        // `poll_ready` is where a tower limiter usually waits, and waiting is
        // what this must not do — see the module docstring. It is also the
        // wrong place for a *refusal*: `poll_ready` answers about the service,
        // not about one request, and a service that reports itself not-ready
        // makes hyper stop reading the connection rather than answer the
        // caller.
        let held = match Arc::clone(&self.permits).try_acquire_owned() {
            Ok(permit) => permit,
            // `Closed` cannot happen — nothing closes this semaphore — and is
            // matched rather than unwrapped so that a future change which does
            // close it refuses requests instead of panicking in a handler.
            Err(TryAcquireError::NoPermits | TryAcquireError::Closed) => {
                return Box::pin(async { Ok(refusal()) });
            }
        };
        let call = self.inner.call(request);
        Box::pin(async move {
            let response = call.await?;
            let (parts, body) = response.into_parts();
            Ok(http::Response::from_parts(
                parts,
                tonic::body::Body::new(Held { inner: body, held }),
            ))
        })
    }
}

/// The response a caller over the cap gets.
///
/// A gRPC failure is an HTTP **200** whose `grpc-status` header says otherwise
/// — the "trailers-only" response — which is why this is not a 429. A 429 here
/// would reach the client as a transport error with no status code, and every
/// client in this repository turns that into a different exception than the
/// one it has for a refused request.
fn refusal() -> http::Response<tonic::body::Body> {
    let mut response = http::Response::new(tonic::body::Body::empty());
    response.headers_mut().insert(
        "content-type",
        http::HeaderValue::from_static("application/grpc"),
    );
    response
        .headers_mut()
        .insert("grpc-status", http::HeaderValue::from_static(REFUSED));
    response.headers_mut().insert(
        "grpc-message",
        http::HeaderValue::from_static(
            "the node is already serving its configured number of open response \
             streams; retry",
        ),
    );
    response
}

/// A response body that holds a permit until it is done.
///
/// The permit is a field and is never read: dropping it is the whole
/// behaviour, and it is dropped when the body is — at the end of the stream,
/// or when the caller disconnects and hyper drops the response. That is the
/// property `max_concurrent_requests` does not have, stated as a type.
struct Held {
    inner: tonic::body::Body,
    /// Returned to the semaphore by `Drop`. Named rather than `_held` because
    /// a leading underscore on a field reads as "unused, ignore it", and this
    /// one is the point of the struct.
    #[expect(dead_code, reason = "the permit's Drop is what bounds the streams")]
    held: OwnedSemaphorePermit,
}

impl http_body::Body for Held {
    type Data = <tonic::body::Body as http_body::Body>::Data;
    type Error = <tonic::body::Body as http_body::Body>::Error;

    fn poll_frame(
        mut self: core::pin::Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        // `Pin::new` rather than a projection crate, for the reason
        // `observe.rs`'s `Trailing` gives at length: `tonic::body::Body` holds
        // a `Pin<Box<..>>` and so is `Unpin`, and an `OwnedSemaphorePermit` is
        // too.
        core::pin::Pin::new(&mut self.inner).poll_frame(context)
    }

    /// Delegated, not defaulted — the same reasoning as `Trailing`'s.
    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}
