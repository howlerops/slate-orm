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
import { existsSync, mkdtempSync, writeFileSync } from "node:fs";
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

function binary(): string {
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
