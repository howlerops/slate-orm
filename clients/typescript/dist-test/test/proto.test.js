import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
/**
 * The bundled `.proto` must match the one the server is built from.
 *
 * This package ships its own copy so that installing it does not require the
 * Rust tree — `@grpc/proto-loader` reads the file at runtime, so it has to be
 * in the tarball. A copy can drift from its source, and a drifted protocol
 * definition fails as a wrong answer rather than as a build error, which is
 * the worst way for it to fail. So the copy is checked rather than trusted.
 *
 * When this fails, copy the file across; do not edit the copy.
 */
function repositoryRoot() {
    let dir = path.dirname(fileURLToPath(import.meta.url));
    for (;;) {
        if (existsSync(path.join(dir, "Cargo.toml")) && existsSync(path.join(dir, "crates"))) {
            return dir;
        }
        const parent = path.dirname(dir);
        if (parent === dir)
            throw new Error("no repository root found");
        dir = parent;
    }
}
test("the bundled proto matches the server's", () => {
    const root = repositoryRoot();
    const here = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
    const packageRoot = existsSync(path.join(here, "proto"))
        ? here
        : path.join(root, "clients", "typescript");
    for (const relative of [
        path.join("slate", "v1", "records.proto"),
        path.join("google", "rpc", "status.proto"),
        path.join("google", "rpc", "error_details.proto"),
    ]) {
        const bundled = readFileSync(path.join(packageRoot, "proto", relative), "utf8");
        const source = readFileSync(path.join(root, "crates", "slate-server", "proto", relative), "utf8");
        assert.equal(bundled, source, `${relative} has drifted from crates/slate-server/proto; copy it across`);
    }
});
