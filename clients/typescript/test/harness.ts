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

import { Client, type Identity } from "../src/index.js";

export const CONFIG = `
[listen]
address = "127.0.0.1:0"

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

const ROOT = repositoryRoot();

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
function binary(): string {
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
  client(identity?: Identity): Client;
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
    client(identity = APP) {
      const c = Client.connect(address, identity);
      clients.push(c);
      return c;
    },
    stop() {
      for (const c of clients) c.close();
      child.kill();
    },
  };
}
