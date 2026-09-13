//! The generated wire types.
#![allow(clippy::all, clippy::pedantic, missing_docs, unreachable_pub)]
tonic::include_proto!("slate.v1");

/// `google.rpc.Status` and `google.rpc.ErrorInfo`, the standard shape for
/// machine-readable error detail.
///
/// Declared in this repository's own `proto/google/rpc/` rather than pulled in
/// as a dependency: they are twelve lines between them, the build is hermetic
/// on purpose (`protox` parses the schema in Rust so nothing has to have
/// `protoc` installed), and a client decodes them with its own generated
/// copies anyway — the bytes are what agree, not the crate. See
/// [`crate::status`].
pub mod rpc {
    #![allow(clippy::all, clippy::pedantic, missing_docs, unreachable_pub)]
    tonic::include_proto!("google.rpc");
}
