/**
 * The explorer's HTTP contract (`examples/explorer/CONTRACT.md`) as a
 * Cloudflare Worker: the TypeScript client, over gRPC-web, on `workerd`.
 *
 * The endpoints are the Node adapter's own `adapter.ts`, unchanged, so the
 * conformance runner can hold this Worker to the same cases, byte for byte,
 * as the Python, Go and Node adapters. What differs is everything under
 * them — the runtime (`workerd`, not Node), the transport (gRPC-web over
 * `fetch`, not HTTP/2), and the client entry point (`@slate-orm/client/edge`,
 * which loads nothing from Node). That is the point: a Worker that passes the
 * same cases is a client that works on the edge, not one that was tested for
 * it. `docs/edge-client.md` §4.
 *
 * One `Adapter` per isolate, built on the first request: it holds a client
 * per identity, and a Worker isolate is reused across requests. `HEAD` is the
 * head node's base URL.
 */
import { Client } from "@slate-orm/client/edge";

import { Adapter, handle } from "../../explorer/backends/node/src/adapter.js";

interface Env {
  HEAD: string;
}

let adapter: Adapter | undefined;

/**
 * A `fetch` that reports each call's RPC name before sending it, which is how
 * `/api/round-trips` counts over gRPC-web: the name is the last segment of the
 * path, `/slate.v1.Records/Insert`.
 */
function counting(count: (method: string) => void) {
  return (input: string, init: RequestInit): Promise<Response> => {
    count(input.split("/").pop() ?? "?");
    return fetch(input, init);
  };
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    adapter ??= new Adapter((identity, count) =>
      Client.connectWeb(env.HEAD, identity, count ? { fetch: counting(count) } : {}),
    );
    const url = new URL(request.url);
    const answer = await handle(adapter, {
      method: request.method,
      path: url.pathname,
      persona: request.headers.get("x-demo-identity") ?? undefined,
      body: request.method === "OPTIONS" ? "" : await request.text(),
    });
    return new Response(JSON.stringify(answer.body), {
      status: answer.status,
      headers: {
        "content-type": "application/json",
        "access-control-allow-origin": "*",
        "access-control-allow-headers": "content-type, x-demo-identity",
      },
    });
  },
};
