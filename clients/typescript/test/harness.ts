/**
 * Starting a real head node, and connecting to it.
 *
 * Every test here runs against `slate-serverd` built from this repository and
 * started as a subprocess — no mock, for the reason the Python client's
 * conftest gives: a mock is a second statement of what the server does,
 * written by whoever wrote the client, so it agrees with the client's
 * misunderstandings. Finding those is the point of a third client.
 *
 * The daemon prints `LISTENING <addr>` once its listener is bound and before
 * it serves, so this waits for that line rather than polling the port, which
 * closes the race where a connection arrives between bind and accept.
 */
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import readline from "node:readline";

// `grpc` through the client rather than from `@grpc/grpc-js` directly:
// depending on it here would install a second copy, and the objects cross
// the boundary. The re-export exists for exactly this.
import { Client, grpc, type Identity } from "../src/index.js";
import { CONVERSION, method } from "../src/transport.js";

/**
 * Which transport the suite runs over: `native` (grpc-js, the default) or
 * `web` (gRPC-web over `fetch`, the edge client). The **same tests** run over
 * both — `SLATE_TRANSPORT=web npm test` — because the claim is that a `Client`
 * behaves identically over either, and a suite written for the web transport
 * alone would test the cases somebody thought of. `docs/edge-client.md`.
 */
export const TRANSPORT = process.env.SLATE_TRANSPORT === "web" ? "web" : "native";
if (process.env.SLATE_TRANSPORT && !["web", "native"].includes(process.env.SLATE_TRANSPORT)) {
  throw new Error(`SLATE_TRANSPORT=${process.env.SLATE_TRANSPORT}: want "native" or "web"`);
}

/**
 * Sees every request a client sends: the RPC's name (`Insert`, `Query`), the
 * request message, and its serialized size. Transport-neutral, so a test that
 * inspects the wire runs over both transports: a grpc-js interceptor under
 * `native`, a `fetch` wrapper that decodes the outgoing frame under `web`.
 */
export type Observer = (method: string, message: unknown, bytes: number) => void;

export const CONFIG = `
[listen]
address = "127.0.0.1:0"
${TRANSPORT === "web" ? "grpc_web = true" : ""}

[auth]
mode = "trusted-header"

[storage]
backend = "memory"

[[tables]]
name = "docs"
id = 1
columns = [
  { name = "id",   type = "u64" },
  { name = "kind", type = "str" },
  { name = "size", type = "i64" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["docs"]
actions = ["everything"]
`;

const HERE = path.dirname(fileURLToPath(import.meta.url));

/**
 * The repository root, found by walking up.
 *
 * Not a fixed number of `..` segments: this file runs from `test/` in source
 * and from `dist/test/` once compiled, so a relative depth is right in exactly
 * one of those and silently wrong in the other.
 */
function repositoryRoot(): string {
  let dir = HERE;
  for (;;) {
    if (existsSync(path.join(dir, "Cargo.toml")) && existsSync(path.join(dir, "crates"))) {
      return dir;
    }
    const parent = path.dirname(dir);
    if (parent === dir) throw new Error(`no repository root above ${HERE}`);
    dir = parent;
  }
}

/** Exported so a test does not recompute it from a relative depth, which
 * the comment above `repositoryRoot` warns is right from one of `test/` and
 * `dist-test/test/` and silently wrong from the other. */
export const ROOT = repositoryRoot();

let built = false;

/**
 * The daemon to run: built from this tree, or one already built.
 *
 * `cargo build` by default, so the suite tests the daemon in this working
 * tree — the only version whose protocol this client was written against.
 *
 * `SLATE_SERVERD` overrides it with a path. Two reasons, and neither is speed:
 * CI builds the daemon once and hands the same binary to all three client
 * suites, and a contributor working only on this client can run the suite with
 * no Rust installed. A path that is set and missing is a hard error — falling
 * back to `cargo` there would quietly test a different binary from the one the
 * caller named.
 */
/**
 * Exported so that the refusal below can be *driven*, not just read.
 *
 * `scripts/test_prebuilt.py` holds every harness that takes `SLATE_SERVERD`
 * to refusing a stale one, and until this was exported it could only see that
 * this file mentions `refuseIfStale` — a harness that defines the function and
 * never calls it passes that. `test/prebuilt.test.ts` calls this with a
 * deliberately stale binary, which is the same path `start()` takes.
 */
export function binary(): string {
  const named = process.env["SLATE_SERVERD"];
  if (named) {
    if (!existsSync(named)) throw new Error(`SLATE_SERVERD=${named} does not exist`);
    refuseIfStale(named);
    return named;
  }
  if (!built) {
    const result = spawnSync(
      "cargo",
      ["build", "-p", "slate-serverd", "--bin", "slate-serverd"],
      { cwd: ROOT, encoding: "utf8" },
    );
    if (result.status !== 0) {
      throw new Error(`building slate-serverd failed:\n${result.stderr}`);
    }
    built = true;
  }
  return path.join(ROOT, "target", "debug", "slate-serverd");
}

/**
 * Refuse a prebuilt binary older than the source it was built from.
 *
 * This is here because it happened, in the Python suite: a full run reported
 * 153 passing tests against a server built before that session's changes, so
 * every test of the new behaviour was checking the old server and passing,
 * because the client asked for something the old binary politely ignored.
 * Three new tests failing after a rebuild is what found it, which is luck
 * rather than a process.
 *
 * Modification times are crude and catch the whole of the real failure: a
 * binary CI just handed over is minutes old, and one built last week is not.
 */
function refuseIfStale(binaryPath: string): void {
  const built = statSync(binaryPath).mtimeMs;
  let newest = 0;
  let newestPath = "";
  const walk = (dir: string): void => {
    let entries;
    try {
      entries = readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        // `target/` is build output, and the biggest directory in the tree.
        if (entry.name !== "target") walk(full);
        continue;
      }
      if (!/\.(rs|toml|proto)$/.test(entry.name)) continue;
      const stamp = statSync(full).mtimeMs;
      if (stamp > newest) {
        newest = stamp;
        newestPath = full;
      }
    }
  };
  for (const dir of ["crates", path.join("clients", "python", "testserver")]) {
    walk(path.join(ROOT, dir));
  }
  if (!newestPath || built >= newest) return;
  throw new Error(
    `SLATE_SERVERD=${binaryPath} was built before ` +
      `${path.relative(ROOT, newestPath)} was last changed, so the suite would ` +
      `test a server this tree did not produce. Rebuild it, or unset ` +
      `SLATE_SERVERD to build from source.`,
  );
}

export interface Serving {
  readonly address: string;
  /**
   * A client on this node.
   *
   * `observe` sees every request the client sends. It is here because some
   * properties of a client are properties of the *request* and not of the
   * answer — a dropped freshness floor returns exactly the right rows — so the
   * only place to assert them is the wire. The Go suite has done this since
   * 2026-09-16; this client had no way in until
   * `ledger/2026-09-28-the-third-client-counts-and-the-go-instrument-was-half-blind.md`
   * added an interceptor argument. It was grpc-js's `ChannelOptions` until the
   * web transport, which has no interceptors; an {@link Observer} is the same
   * question asked of either transport.
   */
  client(identity?: Identity, observe?: Observer): Client;
  stop(): void;
}

const APP: Identity = { principal: "u64:1", tenant: "u64:1", roles: ["app"] };

/** Run a daemon on an ephemeral port and wait for it to be listening. */
export async function start(extra = ""): Promise<Serving> {
  const dir = mkdtempSync(path.join(tmpdir(), "slate-ts-"));
  const file = path.join(dir, "head.toml");
  writeFileSync(file, CONFIG + extra);

  const child: ChildProcess = spawn(binary(), ["--config", file], {
    stdio: ["ignore", "pipe", "inherit"],
  });

  const address = await new Promise<string>((resolve, reject) => {
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error("slate-serverd never said it was listening"));
    }, 30_000);
    const lines = readline.createInterface({ input: child.stdout! });
    lines.on("line", (line) => {
      if (line.startsWith("LISTENING ")) {
        clearTimeout(timer);
        resolve(line.slice("LISTENING ".length).trim());
      }
    });
    child.on("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`slate-serverd exited with ${code} before listening`));
    });
  });

  const clients: Client[] = [];
  return {
    address,
    client(identity = APP, observe?: Observer) {
      const c =
        TRANSPORT === "web"
          ? Client.connectWeb(`http://${address}`, identity, observe ? { fetch: observing(observe) } : {})
          : Client.connect(
              address,
              identity,
              grpc.credentials.createInsecure(),
              observe ? { interceptors: [intercepting(observe)] } : {},
            );
      clients.push(c);
      return c;
    },
    stop() {
      for (const c of clients) c.close();
      child.kill();
    },
  };
}

/**
 * An {@link Observer} as a grpc-js interceptor. The size is the channel's own
 * serializer's, the exact buffer grpc-js is about to send.
 */
function intercepting(observe: Observer) {
  return (options: grpc.InterceptorOptions, nextCall: grpc.NextCall) => {
    const name = options.method_definition.path.split("/").pop() ?? "?";
    return new grpc.InterceptingCall(nextCall(options), {
      sendMessage(message: unknown, next: (message: unknown) => void) {
        observe(name, message, options.method_definition.requestSerialize(message).length);
        next(message);
      },
    });
  };
}

/**
 * An {@link Observer} as a `fetch` wrapper: the RPC's name from the URL, and
 * the request decoded out of the frame the web transport is about to post.
 * The size is the frame's payload, the same bytes `requestSerialize` weighs.
 */
function observing(observe: Observer) {
  return (input: string, init: RequestInit): Promise<Response> => {
    const name = input.split("/").pop() ?? "?";
    const body = init.body as Uint8Array;
    const payload = body.subarray(5);
    const type = method(name).request;
    observe(name, type.toObject(type.decode(payload), CONVERSION), payload.length);
    return globalThis.fetch(input, init);
  };
}

