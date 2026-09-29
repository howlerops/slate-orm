#!/usr/bin/env python3
"""No library crate acquires a binary's async runtime without saying so.

`slate-kernel` once declared `tokio.workspace = true`, and the workspace
default is `["rt-multi-thread", "macros", "sync"]` — features chosen for the
binaries. Nothing used them. It surfaced only when the crate was first built
for `wasm32-unknown-unknown`, where tokio refuses outright: *"Only features
sync,macros,io-util,rt,time are supported on wasm"*.
`ledger/2026-09-14-the-kernel-on-wasm.md` fixed that one line and left the
caveat this exists for:

> Nothing else in the workspace is checked for the same problem. The other
> library crates … do not depend on tokio, and the rest are binaries or
> backends that want the full runtime, but "I looked and these are fine" is
> weaker than a check, and there is no check.

Two rules, because the hazard has two halves and only one of them a build can
see.

**Every crate `slate-wasm` links must ask tokio for wasm-supported features
only.** That closure is *computed* from the manifests rather than listed here:
a crate entering it is found rather than remembered. CI does build this closure
for `wasm32-unknown-unknown` in the playground job, which is the stronger
check — this one runs in a second with no toolchain, names the offending crate
and feature instead of a `compile_error!` inside tokio, and fires on a manifest
change before anybody waits for a wasm build.

**Every library crate *outside* that closure that takes a non-dev tokio must be
listed in `RUNTIME_ON_PURPOSE`, with a reason.** This is the half no build
catches, and it is the half the caveat is actually about: `slate-kernel` was
outside any wasm build on the day it was wrong. A library that inherits the
workspace default is making a binary's decision on its dependents' behalf, and
whether that is right depends on what the library is for — which is a person's
call, recorded once, not a rule.

A **binary** needs no entry: choosing a runtime is what a binary is for. A
library with no tokio needs no entry either. What costs something is a library
that takes one, which is exactly where the cost should fall.

Run directly: `python3 scripts/check_wasm_runtime.py`.
"""

from __future__ import annotations

import sys
from pathlib import Path

import tomllib

ROOT = Path(__file__).resolve().parent.parent

#: The crate whose dependency closure must stay wasm-clean. Named rather than
#: discovered, because "the crate that targets wasm" is not a fact any manifest
#: states; `crate-type = ["cdylib"]` is the nearest signal and means several
#: other things too.
WASM_CRATE = "slate-wasm"

#: tokio's own list, from the `compile_error!` that started this. Copied rather
#: than derived, and it can go stale in one direction only: tokio *adding* a
#: wasm feature would make this refuse something that now compiles, which is a
#: failure a person reads rather than a silence.
ON_WASM = frozenset({"sync", "macros", "io-util", "rt", "time"})

#: Library crates outside the wasm closure that take a non-dev tokio, and why
#: each is entitled to a runtime a library normally should not assume.
#:
#: Each of these is a *server* in library form — its whole job is to hold a
#: listener, a lease or a load generator — so the runtime is not an inherited
#: accident but the thing the crate is. That is the distinction `slate-kernel`
#: failed: it is a query engine, and a query engine that names a thread pool
#: has made a decision belonging to whoever embeds it.
RUNTIME_ON_PURPOSE: dict[str, str] = {
    "slate-slatedb": (
        "the SlateDB-backed store: it drives object-store I/O and a writer "
        "lease, both of which need a live reactor, and it cannot target wasm "
        "for the same reason"
    ),
    "slate-server": (
        "the gRPC service, in library form so the daemon and the test harness "
        "can both mount it. A service that cannot spawn is not a service"
    ),
    "slate-headbench": (
        "the head-node load generator: it exists to open many connections at "
        "once, which is `rt-multi-thread` by definition"
    ),
}

DEP_SECTIONS = ("dependencies", "build-dependencies")


def members() -> dict[str, Path]:
    """Every workspace member, by package name."""
    root = tomllib.loads((ROOT / "Cargo.toml").read_text())
    found = {}
    for name in root.get("workspace", {}).get("members", []):
        manifest = ROOT / name / "Cargo.toml"
        if manifest.exists():
            found[tomllib.loads(manifest.read_text())["package"]["name"]] = manifest
    return found


def sections(manifest: dict[str, object]) -> list[dict[str, object]]:
    """Every non-dev dependency table, target-scoped ones included.

    `slate-wasm` puts `uuid`'s `js` feature under
    `[target.'cfg(target_arch = "wasm32")'.dependencies]`, so a rule reading
    only the plain table would miss the shape it is most likely to meet.
    """
    tables: list[object] = [manifest.get(name, {}) for name in DEP_SECTIONS]
    scoped = manifest.get("target", {})
    if isinstance(scoped, dict):
        for one in scoped.values():
            if isinstance(one, dict):
                tables.extend(one.get(name, {}) for name in DEP_SECTIONS)
    return [t for t in tables if isinstance(t, dict)]


def closure(start: str, crates: dict[str, Path]) -> set[str]:
    """`start` and every workspace crate it reaches without dev-dependencies."""
    seen, queue = {start}, [start]
    while queue:
        manifest = tomllib.loads(crates[queue.pop()].read_text())
        for table in sections(manifest):
            for name in table:
                if name in crates and name not in seen:
                    seen.add(name)
                    queue.append(name)
    return seen


def tokio_features(manifest: dict[str, object]) -> tuple[list[str], bool] | None:
    """The features a non-dev tokio asks for, and whether defaults come too."""
    for table in sections(manifest):
        spec = table.get("tokio")
        if spec is None:
            continue
        if not isinstance(spec, dict):  # `tokio = "1"`, all defaults
            return [], True
        inherits = bool(spec.get("workspace")) or spec.get("default-features", True)
        return list(spec.get("features", [])), bool(inherits)
    return None


def is_library(manifest: dict[str, object], where: Path) -> bool:
    """A crate with a `src/lib.rs` or an explicit `[lib]`."""
    return "lib" in manifest or (where.parent / "src" / "lib.rs").exists()


def main() -> int:
    problems: list[str] = []
    crates = members()

    if WASM_CRATE not in crates:
        # The never-fires case, and the likeliest one: the crate is renamed and
        # every rule below quietly checks a closure of nothing.
        print(
            f"{WASM_CRATE} is not a workspace member. Either it was renamed or "
            "the wasm build is gone — both need a person, not a pass.",
            file=sys.stderr,
        )
        return 1

    reached = closure(WASM_CRATE, crates)
    if len(reached) < 2:
        problems.append(
            f"{WASM_CRATE} reaches no other workspace crate, which cannot be "
            "right — it exists to compile the kernel. The dependency parsing "
            "here has stopped working."
        )

    for name in sorted(reached):
        manifest = tomllib.loads(crates[name].read_text())
        asked = tokio_features(manifest)
        if asked is None:
            continue
        features, inherits = asked
        where = crates[name].relative_to(ROOT)
        if inherits:
            problems.append(
                f"{where}: {name} is in {WASM_CRATE}'s dependency closure and "
                "takes tokio's default features.\n"
                "  The workspace default is a binary's — `rt-multi-thread` is "
                "in it and wasm refuses it. Say `default-features = false` and "
                "list what the crate uses, as slate-kernel does."
            )
        for feature in sorted(set(features) - ON_WASM):
            problems.append(
                f"{where}: {name} asks tokio for `{feature}`, which "
                f"wasm32-unknown-unknown does not support (only "
                f"{sorted(ON_WASM)}).\n"
                f"  It is in {WASM_CRATE}'s closure, so the playground build "
                "will fail on a `compile_error!` inside tokio rather than here."
            )

    for name in sorted(set(crates) - reached):
        manifest = tomllib.loads(crates[name].read_text())
        if not is_library(manifest, crates[name]) or tokio_features(manifest) is None:
            continue
        if name not in RUNTIME_ON_PURPOSE:
            problems.append(
                f"{crates[name].relative_to(ROOT)}: {name} is a library crate "
                "that takes a non-dev tokio and is not in RUNTIME_ON_PURPOSE.\n"
                "  A library naming a runtime makes a decision belonging to "
                "whoever embeds it — slate-kernel did, used none of it, and it "
                "took a wasm build to notice. Either narrow the dependency to "
                "what the crate uses, or add it here with a reason the runtime "
                "is the point of the crate."
            )

    for name in sorted(RUNTIME_ON_PURPOSE):
        if name not in crates:
            problems.append(
                f"RUNTIME_ON_PURPOSE names {name}, which is not a workspace "
                "member any more. Drop the entry, or this rule is excusing a "
                "crate that is gone."
            )
        elif name in reached:
            problems.append(
                f"RUNTIME_ON_PURPOSE names {name}, which is now in "
                f"{WASM_CRATE}'s closure. The exemption and the wasm build "
                "cannot both be right: either the dependency is a mistake, or "
                "the crate has to lose the runtime."
            )

    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        print(f"\n{len(problems)} problem(s)", file=sys.stderr)
        return 1

    print(
        f"ok    {len(reached)} crates in {WASM_CRATE}'s closure ask tokio for "
        f"nothing wasm refuses; {len(RUNTIME_ON_PURPOSE)} libraries outside it "
        "hold a runtime on purpose"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
