/**
 * The entry point for runtimes without Node's networking: Cloudflare Workers,
 * Deno, a browser. The same client as `index.ts`, minus the native gRPC
 * transport and the grpc-js re-export, so nothing here loads `node:http2`.
 * Connect with `Client.connectWeb(url, identity)` to a node running with
 * `listen.grpc_web = true`. `docs/edge-client.md`.
 */
export * from "./api.js";
