//! gRPC-web on the node's one port, as an edge runtime's `fetch` would send it.
//!
//! `docs/edge-client.md` §1: `listen.grpc_web = true` makes the node accept
//! HTTP/1.1 and gRPC-web, translated to native gRPC outside every other layer.
//! These requests are written by hand over a plain TCP socket, deliberately: a
//! gRPC-web client library would agree with the server about anything they
//! both got wrong, and the thing worth pinning is the bytes a `fetch` puts on
//! the wire and gets back.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    unreachable_pub
)]

mod harness;

use harness::{Files, Serving, proto};
use prost::Message;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

fn config(grpc_web: bool) -> String {
    format!(
        r#"
[listen]
address = "127.0.0.1:0"
grpc_web = {grpc_web}

[auth]
mode = "trusted-header"

[shutdown]
grace = "2s"

[storage]
backend = "memory"

[[tables]]
name = "docs"
id = 1
columns = [
  {{ name = "id", type = "u64" }},
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["docs"]
actions = ["all"]
"#
    )
}

fn serving(files: &Files, grpc_web: bool) -> Serving {
    let path = files.write("head.toml", &config(grpc_web));
    Serving::start(&["--config", &path.display().to_string()])
}

/// One gRPC-web unary call over HTTP/1.1: the response head (status line
/// and headers, lowercased), and the frames the body carried, as
/// `(flag, payload)`.
fn call(
    serving: &Serving,
    method: &str,
    message: &[u8],
    principal: Option<&str>,
) -> (String, Vec<(u8, Vec<u8>)>) {
    let mut body = vec![0u8];
    body.extend_from_slice(&u32::try_from(message.len()).unwrap().to_be_bytes());
    body.extend_from_slice(message);
    let identity = principal.map_or(String::new(), |p| {
        format!("slate-principal: {p}\r\nslate-tenant: u64:1\r\nslate-roles: app\r\n")
    });
    let request = format!(
        "POST /slate.v1.Records/{method} HTTP/1.1\r\nHost: {}\r\n\
         Content-Type: application/grpc-web+proto\r\nX-Grpc-Web: 1\r\n{identity}\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        serving.address,
        body.len()
    );
    let mut socket = TcpStream::connect(&serving.address).expect("connect");
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    socket.write_all(request.as_bytes()).unwrap();
    socket.write_all(&body).unwrap();
    let mut raw = Vec::new();
    let _ = socket.read_to_end(&mut raw);
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(raw.len());
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let rest = raw.get(split + 4..).unwrap_or_default();
    let payload = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        dechunk(rest)
    } else {
        rest.to_vec()
    };
    (head.to_ascii_lowercase(), frames(&payload))
}

fn dechunk(mut rest: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let Some(end) = rest.windows(2).position(|w| w == b"\r\n") else {
            return out;
        };
        let size =
            usize::from_str_radix(String::from_utf8_lossy(&rest[..end]).trim(), 16).unwrap_or(0);
        if size == 0 {
            return out;
        }
        out.extend_from_slice(&rest[end + 2..end + 2 + size]);
        rest = &rest[end + 2 + size + 2..];
    }
}

fn frames(mut payload: &[u8]) -> Vec<(u8, Vec<u8>)> {
    let mut out = Vec::new();
    while payload.len() >= 5 {
        let flag = payload[0];
        let length = u32::from_be_bytes(payload[1..5].try_into().unwrap()) as usize;
        out.push((flag, payload[5..5 + length].to_vec()));
        payload = &payload[5 + length..];
    }
    out
}

fn trailer(frames: &[(u8, Vec<u8>)]) -> String {
    frames
        .iter()
        .find(|(flag, _)| flag & 0x80 != 0)
        .map(|(_, bytes)| String::from_utf8_lossy(bytes).to_ascii_lowercase())
        .expect("a gRPC-web response ends with a trailer frame")
}

#[test]
fn a_unary_call_over_grpc_web_answers_with_its_status_in_the_body() {
    let files = Files::new();
    let serving = serving(&files, true);
    let (head, frames) = call(&serving, "Leadership", &[], Some("u64:1"));
    assert!(head.starts_with("http/1.1 200"), "{head}");
    let data = frames
        .iter()
        .find(|(flag, _)| *flag == 0)
        .expect("a data frame");
    let leadership =
        proto::LeadershipStatus::decode(data.1.as_slice()).expect("a LeadershipStatus");
    assert_eq!(
        leadership.standing,
        proto::leadership_status::Standing::Leader as i32
    );
    assert!(
        trailer(&frames).contains("grpc-status:0"),
        "{}",
        trailer(&frames)
    );
}

#[test]
fn a_refusal_over_grpc_web_is_a_status_a_fetch_can_read() {
    let files = Files::new();
    let serving = serving(&files, true);
    // No identity headers: the trusted-header authenticator refuses.
    let (head, frames) = call(&serving, "Leadership", &[], None);
    // Answered 200, with the status where `fetch` can read it. **Not** in a
    // trailer frame: a refusal before any message is a *trailers-only*
    // response, and gRPC-web sends those as ordinary HTTP headers with an
    // empty body. So a client has two places to look — the headers first,
    // then the last frame — and the TypeScript transport is written to that;
    // this pins the half that is easy to miss.
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(
        head.contains("grpc-status: 16"),
        "an unauthenticated call, as UNAUTHENTICATED: {head}"
    );
    assert!(
        head.contains("grpc-message: "),
        "the reason travels too, percent-encoded: {head}"
    );
    assert!(
        frames.is_empty(),
        "a trailers-only answer has no body: {frames:?}"
    );
}

/// Off unless written: a node that did not ask for gRPC-web does not speak
/// HTTP/1.1 at all.
#[test]
fn without_the_setting_http1_is_not_served() {
    let files = Files::new();
    let serving = serving(&files, false);
    let (head, frames) = call(&serving, "Leadership", &[], Some("u64:1"));
    assert!(
        !head.starts_with("http/1.1 200"),
        "HTTP/1.1 was served: {head}"
    );
    assert!(frames.is_empty(), "{frames:?}");
}
