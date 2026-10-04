/**
 * The gRPC-web transport against responses written byte by byte.
 *
 * The whole suite already runs over this transport (`SLATE_TRANSPORT=web`),
 * against a real node. What it cannot reach are the shapes the node does not
 * produce on demand, and a mutation found the first of them: every refusal
 * this server sends is *trailers-only* — status in the headers, no body — so
 * treating a failure in the **trailer frame** as success survived all 211
 * tests. A stream that fails after its first message, or a server between the
 * client and a node that chunks differently, takes exactly that path. So these
 * script `fetch` and say what each shape must produce.
 */
import assert from "node:assert/strict";
import { test } from "node:test";

import { Client, isKind } from "../src/index.js";
import { encode, method } from "../src/transport.js";

const ME = { principal: "u64:1" };

function frame(flag: number, payload: Uint8Array): Uint8Array {
  const out = new Uint8Array(5 + payload.length);
  out[0] = flag;
  new DataView(out.buffer).setUint32(1, payload.length);
  out.set(payload, 5);
  return out;
}

function trailer(text: string): Uint8Array {
  return frame(0x80, new TextEncoder().encode(text));
}

function join(parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}

/** A `fetch` that answers every call with `body`, delivered in `chunks`. */
function answering(body: Uint8Array, chunks: number[] = [body.length], headers: Record<string, string> = {}) {
  return async () => {
    let at = 0;
    const stream = new ReadableStream<Uint8Array>({
      pull(controller) {
        const size = chunks.shift() ?? body.length - at;
        if (at >= body.length) {
          controller.close();
          return;
        }
        controller.enqueue(body.slice(at, at + size));
        at += size;
      },
    });
    return new Response(stream, {
      status: 200,
      headers: { "content-type": "application/grpc-web+proto", ...headers },
    });
  };
}

const leaderMessage = () =>
  encode(method("Leadership").response, {
    standing: "STANDING_LEADER",
    holder: "node-a",
  });

test("a message, then OK in the trailer frame, is the answer", async () => {
  const body = join([frame(0, leaderMessage()), trailer("grpc-status:0\r\n")]);
  const client = Client.connectWeb("http://node", ME, { fetch: answering(body) });
  const leadership = await client.leadership();
  assert.equal(leadership.leader, true);
  assert.equal(leadership.holder, "node-a");
});

test("a failure in the trailer frame is a failure, even after a message", async () => {
  // The shape a stream that fails part-way takes: data first, the status last.
  const body = join([
    frame(0, leaderMessage()),
    trailer("grpc-status:13\r\ngrpc-message:the%20node%20gave%20up\r\n"),
  ]);
  const client = Client.connectWeb("http://node", ME, { fetch: answering(body) });
  await assert.rejects(client.leadership(), (error: unknown) => {
    assert.ok(isKind(error, "internal"), `got ${String(error)}`);
    assert.match(String((error as Error).message), /the node gave up/);
    return true;
  });
});

test("frames split at every byte boundary reassemble to the same answer", async () => {
  const body = join([frame(0, leaderMessage()), trailer("grpc-status:0\r\n")]);
  for (let size = 1; size <= 7; size++) {
    const chunks = Array.from({ length: Math.ceil(body.length / size) }, () => size);
    const client = Client.connectWeb("http://node", ME, { fetch: answering(body, chunks) });
    const leadership = await client.leadership();
    assert.equal(leadership.holder, "node-a", `chunks of ${size}`);
  }
});

test("a status in the headers, with no body, is read from the headers", async () => {
  const client = Client.connectWeb("http://node", ME, {
    fetch: answering(new Uint8Array(0), [], {
      "grpc-status": "7",
      "grpc-message": "access%20denied",
    }),
  });
  await assert.rejects(client.leadership(), (error: unknown) => {
    assert.ok(isKind(error, "permission-denied"), `got ${String(error)}`);
    assert.match(String((error as Error).message), /access denied/);
    return true;
  });
});

test("a response that ends with no status is not a success", async () => {
  const client = Client.connectWeb("http://node", ME, {
    fetch: answering(frame(0, leaderMessage())),
  });
  await assert.rejects(client.leadership());
});

test("a node that does not speak gRPC-web says so", async () => {
  const client = Client.connectWeb("http://node", ME, {
    fetch: async () => new Response("no", { status: 415 }),
  });
  await assert.rejects(client.leadership(), (error: unknown) => {
    assert.match(String((error as Error).message), /grpc_web/);
    return true;
  });
});
