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
  /**
   * A Wrangler secret (`wrangler secret put SLATE_TOKENS`): a JSON object from
   * persona to bearer token, `{"app": "…", "reader": "…"}`, for a head node in
   * `[auth] mode = "token"`. Unset, the Worker sends the demo's identity
   * headers alone, which only a `trusted-header` node — one on loopback,
   * under `wrangler dev` — accepts. A node reachable from the internet must
   * not be in that mode: it trusts whatever identity a caller claims.
   */
  SLATE_TOKENS?: string;
}

let adapter: Adapter | undefined;

/**
 * The `fetch` a client for one persona uses: it adds that persona's bearer
 * token when there is one, and reports each call's RPC name when asked to
 * count, which is how `/api/round-trips` counts over gRPC-web (the name is the
 * last segment of the path, `/slate.v1.Records/Insert`).
 */
function sending(token: string | undefined, count: ((method: string) => void) | undefined) {
  return (input: string, init: RequestInit): Promise<Response> => {
    count?.(input.split("/").pop() ?? "?");
    if (token === undefined) return fetch(input, init);
    return fetch(input, {
      ...init,
      headers: { ...(init.headers as Record<string, string>), authorization: `Bearer ${token}` },
    });
  };
}

/** The persona an identity stands for: its role, which the adapter sets to the persona's name. */
function persona(identity: { roles?: readonly string[] }): string {
  return identity.roles?.[0] ?? "";
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const tokens: Record<string, string> = env.SLATE_TOKENS ? JSON.parse(env.SLATE_TOKENS) : {};
    adapter ??= new Adapter((identity, count) =>
      Client.connectWeb(env.HEAD, identity, { fetch: sending(tokens[persona(identity)], count) }),
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
