/**
 * The Node entry point: the whole client, with `Client.connect` speaking native
 * gRPC over `@grpc/grpc-js`. On Cloudflare Workers, Deno or a browser, import
 * `@slate-orm/client/edge` instead and use `Client.connectWeb`.
 */
// First, so `Client.connect` has its transport before anything can call it.
import "./node.js";

export * from "./api.js";
export * as grpc from "@grpc/grpc-js";
