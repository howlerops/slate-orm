/**
 * The gRPC-web transport: every call is one `fetch`.
 *
 * For runtimes whose only network primitive is `fetch` — Cloudflare Workers,
 * Deno, a browser — which cannot read the HTTP/2 trailers native gRPC puts
 * each call's status in. gRPC-web moves the status into the body, and the head
 * node translates when `listen.grpc_web = true` (`docs/edge-client.md`).
 *
 * **The status arrives in one of two places**, and both are read:
 *
 * - **the response headers**, for a *trailers-only* answer: a call refused
 *   before any message — unauthenticated, denied, not found — comes back with
 *   `grpc-status` as an ordinary header and an empty body;
 * - **the last frame of the body**, flagged `0x80`, for an answer that sent
 *   messages first: a stream that fails part-way, and every success.
 *
 * Reading only the trailer frame, which is what the gRPC-web specification's
 * diagram suggests, would turn every refusal into a missing status.
 * `slate-serverd/tests/grpc_web.rs` pins the header case against the server.
 *
 * Imports nothing from Node and nothing from grpc-js; see `transport.ts`.
 */
import {
  type CallFailure,
  type Metadata,
  type MessageStream,
  type Transport,
  callFailure,
  decode,
  encode,
  method,
} from "./transport.js";

/** What a caller may pass in place of the global `fetch`. */
export type Fetch = (input: string, init: RequestInit) => Promise<Response>;

export interface WebOptions {
  /**
   * The `fetch` to call. Defaults to the global one. A Worker passes a
   * service binding's `fetch` here, and a test passes a double.
   */
  fetch?: Fetch;
}

const DATA = 0x00;
const TRAILER = 0x80;

/** gRPC's own codes, by number: `UNKNOWN` and `DEADLINE_EXCEEDED`. */
const UNKNOWN = 2;
const DEADLINE_EXCEEDED = 4;
const UNAVAILABLE = 14;

export class GrpcWebTransport implements Transport {
  readonly #base: string;
  readonly #fetch: Fetch;
  readonly #controllers = new Set<AbortController>();

  /** `base` is the node's URL, `http://host:port`, with no path. */
  constructor(base: string, options: WebOptions = {}) {
    this.#base = base.replace(/\/+$/, "");
    // Bound, because a `fetch` pulled off `globalThis` and called detached
    // throws "Illegal invocation" on Workers and in browsers.
    this.#fetch = options.fetch ?? ((input, init) => globalThis.fetch(input, init));
  }

  async unary(name: string, request: unknown, metadata: Metadata, deadline?: number): Promise<unknown> {
    const rpc = method(name);
    let answer: unknown;
    let answered = false;
    for await (const message of this.#messages(name, rpc, request, metadata, deadline)) {
      answer = message;
      answered = true;
    }
    if (!answered) {
      throw callFailure(UNKNOWN, `${name} finished with status OK and no message`);
    }
    return answer;
  }

  stream(name: string, request: unknown, metadata: Metadata, deadline?: number): MessageStream {
    const rpc = method(name);
    const controller = new AbortController();
    const messages = this.#messages(name, rpc, request, metadata, deadline, controller);
    return {
      cancel: () => controller.abort(),
      [Symbol.asyncIterator]: () => messages[Symbol.asyncIterator](),
    };
  }

  close(): void {
    for (const controller of this.#controllers) controller.abort();
    this.#controllers.clear();
  }

  async *#messages(
    name: string,
    rpc: ReturnType<typeof method>,
    request: unknown,
    metadata: Metadata,
    deadline?: number,
    controller = new AbortController(),
  ): AsyncGenerator<unknown> {
    this.#controllers.add(controller);
    let timer: ReturnType<typeof setTimeout> | undefined;
    let expired = false;
    if (deadline !== undefined) {
      timer = setTimeout(() => {
        expired = true;
        controller.abort();
      }, Math.max(0, deadline - Date.now()));
    }
    try {
      const payload = encode(rpc.request, request);
      const body = new Uint8Array(5 + payload.length);
      body[0] = DATA;
      new DataView(body.buffer).setUint32(1, payload.length);
      body.set(payload, 5);

      const headers: Record<string, string> = {
        "content-type": "application/grpc-web+proto",
        "x-grpc-web": "1",
        ...metadata,
      };
      let response: Response;
      try {
        response = await this.#fetch(`${this.#base}/slate.v1.Records/${name}`, {
          method: "POST",
          headers,
          body,
          signal: controller.signal,
        });
      } catch (error) {
        throw transportFailure(error, expired);
      }

      // A trailers-only answer: the status is a header and there is no body.
      const early = response.headers.get("grpc-status");
      if (early !== null) {
        const code = Number(early);
        if (code !== 0) throw failureFrom(code, response.headers, undefined);
      }
      if (!response.ok) {
        throw callFailure(
          UNAVAILABLE,
          `the node answered HTTP ${response.status} to a gRPC-web call; is listen.grpc_web on?`,
        );
      }
      if (early !== null || response.body === null) return;

      for await (const frame of frames(response.body, expired)) {
        if (frame.flag & TRAILER) {
          const trailers = parseTrailers(frame.payload);
          const code = Number(trailers.get("grpc-status") ?? UNKNOWN);
          if (code !== 0) throw failureFrom(code, response.headers, trailers);
          return;
        }
        yield decode(rpc.response, frame.payload);
      }
      throw callFailure(UNKNOWN, `${name}: the response ended without a status`);
    } catch (error) {
      if (expired && (error as CallFailure).code === undefined) {
        throw callFailure(DEADLINE_EXCEEDED, "deadline exceeded");
      }
      if (expired && (error as CallFailure).code === UNAVAILABLE) {
        throw callFailure(DEADLINE_EXCEEDED, "deadline exceeded");
      }
      throw error;
    } finally {
      if (timer !== undefined) clearTimeout(timer);
      this.#controllers.delete(controller);
    }
  }
}

/** A network-level failure, as the status grpc-js would report for it. */
function transportFailure(error: unknown, expired: boolean): CallFailure {
  if (expired) return callFailure(DEADLINE_EXCEEDED, "deadline exceeded");
  const reason = error instanceof Error ? error.message : String(error);
  return callFailure(UNAVAILABLE, reason);
}

/**
 * The frames of a gRPC-web body, reassembled across however the bytes were
 * chunked: a frame header can straddle two reads, and so can a frame.
 */
async function* frames(
  body: ReadableStream<Uint8Array>,
  expired: boolean,
): AsyncGenerator<{ flag: number; payload: Uint8Array }> {
  const reader = body.getReader();
  let buffered = new Uint8Array(0);
  try {
    for (;;) {
      while (buffered.length >= 5) {
        const length = new DataView(buffered.buffer, buffered.byteOffset).getUint32(1);
        if (buffered.length < 5 + length) break;
        yield { flag: buffered[0]!, payload: buffered.slice(5, 5 + length) };
        buffered = buffered.slice(5 + length);
      }
      let chunk: Awaited<ReturnType<typeof reader.read>>;
      try {
        chunk = await reader.read();
      } catch (error) {
        throw transportFailure(error, expired);
      }
      if (chunk.done) return;
      const joined = new Uint8Array(buffered.length + chunk.value.length);
      joined.set(buffered);
      joined.set(chunk.value, buffered.length);
      buffered = joined;
    }
  } finally {
    reader.releaseLock();
  }
}

/** `key: value` lines, which is what a gRPC-web trailer frame holds. */
function parseTrailers(payload: Uint8Array): Map<string, string> {
  const out = new Map<string, string>();
  for (const line of new TextDecoder().decode(payload).split("\r\n")) {
    const at = line.indexOf(":");
    if (at > 0) out.set(line.slice(0, at).trim().toLowerCase(), line.slice(at + 1).trim());
  }
  return out;
}

/**
 * The failure a status describes, with its metadata as grpc-js would carry it:
 * `grpc-message` percent-decoded into the details, every other text entry
 * kept, and `grpc-status-details-bin` base64-decoded to bytes, which is how
 * `errors.ts` reads the reason token.
 */
function failureFrom(code: number, headers: Headers, trailers: Map<string, string> | undefined): CallFailure {
  const entries: Record<string, string | Uint8Array> = {};
  const add = (key: string, value: string) => {
    if (key === "grpc-status" || key === "grpc-message" || key === "content-type") return;
    entries[key] = key.endsWith("-bin") ? base64(value) : value;
  };
  headers.forEach((value, key) => add(key.toLowerCase(), value));
  trailers?.forEach((value, key) => add(key, value));
  const message = trailers?.get("grpc-message") ?? headers.get("grpc-message") ?? "";
  return callFailure(code, decodeMessage(message), entries);
}

function decodeMessage(message: string): string {
  try {
    return decodeURIComponent(message);
  } catch {
    return message;
  }
}

/** Standard or unpadded base64, which is how gRPC sends a `-bin` entry. */
function base64(value: string): Uint8Array {
  const padded = value + "=".repeat((4 - (value.length % 4)) % 4);
  const binary = atob(padded);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}
