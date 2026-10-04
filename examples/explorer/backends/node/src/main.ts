/**
 * The explorer's Node adapter: `CONTRACT.md` over `node:http`, with the
 * TypeScript client speaking native gRPC.
 *
 * Everything the endpoints do is in `adapter.ts`, which imports nothing from
 * Node and is shared with the Worker in `examples/edge` — the same contract,
 * the same client, over gRPC-web on `workerd`. This file is the Node half:
 * read a body, hand it over, write the answer.
 *
 * `node:http` rather than a framework: a handful of endpoints and no
 * middleware.
 */
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { parseArgs } from "node:util";

// The Node entry point, which installs the native gRPC transport that
// `Client.connect` uses. `adapter.ts` imports the edge entry; both are the same
// client, and this import is what gives it a transport.
import { Client, grpc } from "@slate-orm/client";

import { Adapter, handle } from "./adapter.js";

async function readBody(request: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of request) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks).toString("utf8");
}

function send(response: ServerResponse, status: number, body: unknown): void {
  const payload = JSON.stringify(body);
  response.writeHead(status, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(payload),
    "access-control-allow-origin": "*",
    "access-control-allow-headers": "content-type, x-demo-identity",
  });
  response.end(payload);
}

/**
 * A count callback as a grpc-js interceptor. One covers both a unary write
 * and a server-streaming read, because grpc-js runs `interceptors` on every
 * call regardless of its shape — which is not true of the other two clients:
 * Go needs a unary *and* a stream interceptor, and the Go adapter's counter
 * shipped with only the first for a day.
 */
function counting(count: (method: string) => void) {
  return (options: grpc.InterceptorOptions, nextCall: grpc.NextCall) => {
    count(options.method_definition.path.split("/").pop() ?? "?");
    return new grpc.InterceptingCall(nextCall(options));
  };
}

async function main(): Promise<void> {
  const { values } = parseArgs({
    options: {
      head: { type: "string", default: "127.0.0.1:7421" },
      listen: { type: "string", default: "127.0.0.1:7432" },
    },
    allowPositionals: true,
  });
  const head = values.head!;
  const adapter = new Adapter((identity, count) =>
    count
      ? Client.connect(head, identity, grpc.credentials.createInsecure(), {
          interceptors: [counting(count)],
        })
      : Client.connect(head, identity),
  );

  const server = createServer((request, response) => {
    void (async () => {
      const answer = await handle(adapter, {
        method: request.method ?? "GET",
        path: (request.url ?? "").split("?")[0] ?? "",
        persona: request.headers["x-demo-identity"] as string | undefined,
        body: request.method === "OPTIONS" ? "" : await readBody(request),
      });
      send(response, answer.status, answer.body);
    })();
  });

  const [host, port] = values.listen!.split(":");
  server.listen(Number(port), host, () => {
    // The same handshake the head node uses, so `run.sh` can wait for a line
    // rather than poll a port.
    console.log(`LISTENING ${values.listen}`);
  });
}

void main();
