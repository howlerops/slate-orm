#!/usr/bin/env python3
"""Tests for `check_wasm_runtime.py`, over manifests this one writes.

Run against the real workspace, a rule that had stopped checking would pass for
as long as the crates stayed correct — which is the failure mode the guard
exists to prevent, one level up. The case that matters most is the one the
caveat is about: a library crate outside the wasm build taking a runtime, which
is what `slate-kernel` did and what no wasm build could have caught.

Run directly: `python3 scripts/test_check_wasm_runtime.py`.
"""

from __future__ import annotations

import contextlib
import io
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_wasm_runtime as guard

#: A miniature of the real shape: a wasm crate over a portable kernel, and a
#: server-shaped library outside the closure that holds a runtime on purpose.
CRATES = {
    "slate-wasm": """\
[package]
name = "slate-wasm"
[lib]
crate-type = ["cdylib"]
[dependencies]
slate-kernel = { path = "../slate-kernel" }
[target.'cfg(target_arch = "wasm32")'.dependencies]
uuid = { workspace = true, features = ["js"] }
[dev-dependencies]
tokio = { workspace = true, features = ["rt-multi-thread", "macros"] }
""",
    "slate-kernel": """\
[package]
name = "slate-kernel"
[dependencies]
tokio = { version = "1", default-features = false, features = ["time"] }
""",
    "slate-server": """\
[package]
name = "slate-server"
[dependencies]
tokio = { workspace = true, features = ["time", "rt"] }
""",
    "slate-serverd": """\
[package]
name = "slate-serverd"
[dependencies]
tokio = { workspace = true, features = ["rt-multi-thread", "macros"] }
""",
}

#: The roster the fixture is checked against, replacing the real one for the
#: duration. Patched rather than reused: the real `RUNTIME_ON_PURPOSE` names
#: crates this miniature does not have, and a test that had to grow a fixture
#: crate every time somebody exempted a real one would be tracking the roster
#: rather than the rules.
ROSTER = {"slate-server": "the gRPC service in library form, for the fixture"}

#: `slate-serverd` has a `src/main.rs` and no `src/lib.rs`, so it is a binary
#: and needs no entry. The fixture has to build the files, because that is how
#: the guard tells a library from a binary.
BINARIES = {"slate-serverd"}


def tree(root: pathlib.Path, crates: dict[str, str] | None = None) -> None:
    crates = CRATES if crates is None else crates
    names = ",\n  ".join(f'"crates/{name}"' for name in crates)
    (root / "Cargo.toml").write_text(f"[workspace]\nmembers = [\n  {names}\n]\n")
    for name, manifest in crates.items():
        where = root / "crates" / name
        (where / "src").mkdir(parents=True)
        (where / "Cargo.toml").write_text(manifest)
        leaf = "main.rs" if name in BINARIES else "lib.rs"
        (where / "src" / leaf).write_text("")


def run(root: pathlib.Path) -> tuple[int, str]:
    """The guard's exit code and everything it printed.

    A raise is turned into a failing case rather than allowed to end the run.
    Found by mutating: making the missing-wasm-crate arm unreachable left the
    closure walk to `KeyError`, which killed this harness before it printed a
    tally — and `mutate.py` read that as NOTHING RAN, not as a survivor. A
    harness that cannot report on broken code cannot score a mutation, which is
    the same lie the conformance runner was fixed for.
    """
    guard.ROOT, guard.RUNTIME_ON_PURPOSE = root, dict(ROSTER)
    out, err = io.StringIO(), io.StringIO()
    try:
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = guard.main()
    except Exception as raised:  # noqa: BLE001 - any raise is a failing case
        return 70, f"{out.getvalue()}{err.getvalue()}the guard raised {raised!r}"
    return code, out.getvalue() + err.getvalue()


def clean(root: pathlib.Path) -> None:
    tree(root)


def closure_crate_inherits(root: pathlib.Path) -> None:
    """The exact mistake the entry fixed: the kernel taking workspace defaults."""
    crates = dict(CRATES)
    crates["slate-kernel"] = crates["slate-kernel"].replace(
        'tokio = { version = "1", default-features = false, features = ["time"] }',
        'tokio = { workspace = true, features = ["time"] }',
    )
    tree(root, crates)


def closure_crate_asks_for_a_thread_pool(root: pathlib.Path) -> None:
    crates = dict(CRATES)
    crates["slate-kernel"] = crates["slate-kernel"].replace(
        'features = ["time"]', 'features = ["time", "rt-multi-thread"]'
    )
    tree(root, crates)


def unlisted_library_takes_a_runtime(root: pathlib.Path) -> None:
    """The half no wasm build can catch, and the half the caveat is about."""
    crates = dict(CRATES)
    crates["slate-sql"] = '[package]\nname = "slate-sql"\n[dependencies]\ntokio = "1"\n'
    tree(root, crates)


def unlisted_binary_takes_a_runtime(root: pathlib.Path) -> None:
    """And the half that must *not* fire: a binary choosing its own runtime."""
    tree(root)


def exemption_for_a_crate_that_is_gone(root: pathlib.Path) -> None:
    crates = {k: v for k, v in CRATES.items() if k != "slate-server"}
    tree(root, crates)


def exemption_for_a_crate_now_in_the_closure(root: pathlib.Path) -> None:
    crates = dict(CRATES)
    crates["slate-wasm"] = crates["slate-wasm"].replace(
        'slate-kernel = { path = "../slate-kernel" }',
        'slate-kernel = { path = "../slate-kernel" }\nslate-server = { path = "../slate-server" }',
    )
    tree(root, crates)


def a_workspace_crate_behind_a_cfg(root: pathlib.Path) -> None:
    """A workspace dependency in a target table is still in the closure.

    `slate-wasm` uses a target table today, for `uuid`'s `js` feature, but no
    *workspace* crate sits behind a `cfg` — so skipping those tables changed
    nothing and the mutation survived. This is the shape that distinguishes
    them, and it has to be contrived: the crate asks for a feature wasm refuses
    *and* sits in the roster, because that is the only combination where the
    two rules give different answers. In the closure it is a bad feature; out
    of it, an excused library. A crate that were only one of those would fail
    either way and the case would see nothing.
    """
    crates = dict(CRATES)
    crates["slate-wasm"] = crates["slate-wasm"].replace(
        'uuid = { workspace = true, features = ["js"] }',
        'uuid = { workspace = true, features = ["js"] }\n'
        'slate-server = { path = "../slate-server" }',
    )
    crates["slate-server"] = crates["slate-server"].replace(
        'tokio = { workspace = true, features = ["time", "rt"] }',
        'tokio = { version = "1", default-features = false, features = ["net"] }',
    )
    tree(root, crates)


def the_wasm_crate_is_gone(root: pathlib.Path) -> None:
    tree(root, {k: v for k, v in CRATES.items() if k != "slate-wasm"})


def the_wasm_crate_reaches_nothing(root: pathlib.Path) -> None:
    """The never-fires case: a closure of one checks one manifest and passes."""
    crates = dict(CRATES)
    crates["slate-wasm"] = crates["slate-wasm"].replace(
        'slate-kernel = { path = "../slate-kernel" }\n', ""
    )
    tree(root, crates)


CASES = [
    ("the real shape passes", clean, 0, "2 crates in slate-wasm's closure"),
    (
        "a closure crate inheriting workspace defaults fails",
        closure_crate_inherits,
        1,
        "takes tokio's default features",
    ),
    (
        "a closure crate asking for rt-multi-thread fails",
        closure_crate_asks_for_a_thread_pool,
        1,
        "asks tokio for `rt-multi-thread`",
    ),
    (
        "an unlisted library taking a runtime fails",
        unlisted_library_takes_a_runtime,
        1,
        "not in RUNTIME_ON_PURPOSE",
    ),
    ("an unlisted binary taking a runtime passes", unlisted_binary_takes_a_runtime, 0, "closure"),
    (
        "an exemption for a crate that is gone fails",
        exemption_for_a_crate_that_is_gone,
        1,
        "not a workspace member any more",
    ),
    (
        "an exemption for a crate now in the closure fails",
        exemption_for_a_crate_now_in_the_closure,
        1,
        "now in slate-wasm's closure",
    ),
    (
        "a workspace crate behind a cfg is in the closure",
        a_workspace_crate_behind_a_cfg,
        1,
        "asks tokio for `net`",
    ),
    ("a missing wasm crate fails", the_wasm_crate_is_gone, 1, "is not a workspace member"),
    (
        "a wasm crate reaching nothing fails",
        the_wasm_crate_reaches_nothing,
        1,
        "reaches no other workspace crate",
    ),
]


def main() -> int:
    was = (guard.ROOT, guard.RUNTIME_ON_PURPOSE)
    failures = []
    try:
        for name, build, wanted, needle in CASES:
            with tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                build(root)
                code, output = run(root)
            if code != wanted:
                failures.append(f"FAIL  {name}: exit {code}, wanted {wanted}\n{output}")
            elif needle not in output:
                failures.append(f"FAIL  {name}: no {needle!r} in\n{output}")
            else:
                print(f"ok    {name}")
    finally:
        guard.ROOT, guard.RUNTIME_ON_PURPOSE = was

    for failure in failures:
        print(failure, file=sys.stderr)
    print(f"{len(CASES) - len(failures)} passed, {len(failures)} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
