// N single inserts against one batch of N, through the TypeScript SDK.
//
// The shape slate-headbench uses, from a client instead of from the wire: the
// multiplier a client sees is the wire's minus the per-operation work a client
// does and a batch does not save. Reports a median and a range over five runs,
// and does not assert — see ../README.md.
import { Client, str, uint, type Value } from "@slate-orm/client";

function arg(name: string, fallback?: string): string {
  const at = process.argv.indexOf(`--${name}`);
  if (at >= 0 && process.argv[at + 1] !== undefined) return process.argv[at + 1]!;
  if (fallback !== undefined) return fallback;
  throw new Error(`--${name} is required`);
}

async function main(): Promise<void> {
  const address = arg("address");
  const rows = Number(arg("rows", "100"));
  const runs = Number(arg("runs", "5"));

  const client = Client.connect(address, {
    principal: "u64:1",
    tenant: "u64:1",
    roles: ["app"],
  });
  const session = client.session();
  const base = 3_000_000;

  const singles: number[] = [];
  const batches: number[] = [];
  for (let run = 0; run < runs; run += 1) {
    // Disjoint key ranges per run and per arm: an insert refuses a taken key,
    // and a second pass over the same keys would time the refusal.
    const at = base + run * rows * 2;
    singles.push(await timeSingles(session, at, rows));
    batches.push(await timeBatch(session, at + rows, rows));
  }
  client.close();
  report("typescript", rows, singles, batches);
}

async function timeSingles(session: ReturnType<Client["session"]>, base: number, rows: number) {
  const start = process.hrtime.bigint();
  for (let n = 0; n < rows; n += 1) {
    await session.insert("bench", [uint(base + n), str("row")]);
  }
  return Number(process.hrtime.bigint() - start) / 1e9;
}

async function timeBatch(session: ReturnType<Client["session"]>, base: number, rows: number) {
  const inserts: Value[][] = [];
  for (let n = 0; n < rows; n += 1) inserts.push([uint(base + n), str("row")]);
  const start = process.hrtime.bigint();
  await session.batch({
    atomicity: "all-or-nothing",
    operations: [{ kind: "insert", table: "bench", rows: inserts }],
  });
  return Number(process.hrtime.bigint() - start) / 1e9;
}

function report(name: string, rows: number, singles: number[], batches: number[]): void {
  const per = (xs: number[]) => xs.map((x) => (x / rows) * 1e6).sort((a, b) => a - b);
  const median = (xs: number[]) =>
    xs.length % 2 === 1 ? xs[(xs.length - 1) / 2]! : (xs[xs.length / 2 - 1]! + xs[xs.length / 2]!) / 2;
  const s = per(singles);
  const b = per(batches);
  const f = (x: number) => x.toFixed(1);
  console.log(
    [
      name,
      rows,
      f(median(s)),
      f(s[0]!),
      f(s[s.length - 1]!),
      f(median(b)),
      f(b[0]!),
      f(b[b.length - 1]!),
      f(median(s) / median(b)),
    ].join("\t"),
  );
}

main().catch((error: unknown) => {
  console.error(error);
  process.exit(1);
});
