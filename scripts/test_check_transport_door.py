#!/usr/bin/env python3
"""Tests for `check_transport_door.py`, over a tree this one builds.

Run against the real clients, a rule that had stopped checking would pass for
as long as they stayed correct — which is the failure mode the guard exists to
prevent, one level up. The case that matters most is the fourth client: the
guard's whole claim is that it sees one arriving, and a hand-written roster
would not.

Run directly: `python3 scripts/test_check_transport_door.py`.
"""

from __future__ import annotations

import contextlib
import io
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_transport_door as guard


def tree(root: pathlib.Path) -> None:
    """A miniature of the real layout: three clients, three doors, three uses."""
    for name, door in guard.DOORS.items():
        manifest = {"python": "pyproject.toml", "go": "go.mod", "typescript": "package.json"}
        (root / "clients" / name).mkdir(parents=True)
        (root / "clients" / name / manifest[name]).write_text("")
        write(root / door.entry, opening(door.opens))
        write(root / door.walks, opening(door.through))
    write(root / guard.REEXPORT[0], 'export * as grpc from "@grpc/grpc-js";\n')


def opening(pattern: str) -> str:
    """A line the guard's pattern matches.

    The patterns are regexes over source, so the fixture cannot just be the
    pattern string: `\\.` and `\\[` would not match themselves. Unescaping the
    handful of metacharacters these patterns use is enough and keeps the
    fixture honest — it is derived from the rule, so a rule that stops matching
    its own declaration is a test failure rather than a fixture to update.
    """
    for escaped, literal in (("\\.", "."), ("\\|", "|"), ("\\(", "("), ("\\)", ")"),
                             ("\\[", "["), ("\\{", "{"), ("\\}", "}"), ("\\.\\.\\.", "...")):
        pattern = pattern.replace(escaped, literal)
    return pattern.replace("(?:Unary|Stream)", "Unary") + "\n"


def write(path: pathlib.Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def run(root: pathlib.Path) -> tuple[int, str]:
    guard.ROOT, guard.CLIENTS = root, root / "clients"
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = guard.main()
    return code, out.getvalue() + err.getvalue()


def clean(root: pathlib.Path) -> None:
    """Three clients, three doors, three uses."""
    tree(root)


def fourth_client(root: pathlib.Path) -> None:
    """A Rust client arrives with a manifest and no door. The headline case."""
    tree(root)
    (root / "clients" / "rust").mkdir()
    (root / "clients" / "rust" / "Cargo.toml").write_text("")


def no_manifest(root: pathlib.Path) -> None:
    """A directory the discovery cannot classify is a person's problem."""
    tree(root)
    (root / "clients" / "kotlin").mkdir()


def door_removed(root: pathlib.Path) -> None:
    tree(root)
    write(root / guard.DOORS["typescript"].entry, "static connect(target, identity) {}\n")


def entry_file_gone(root: pathlib.Path) -> None:
    """The file declaring the door is renamed away, not edited.

    Written because a mutation making `missing()` return `None` for an absent
    file survived every case above: each of them rewrites a file, so none
    distinguished "the pattern is not there" from "the file is not there", and
    a renamed `client.go` would have read as a door in place.
    """
    tree(root)
    (root / guard.DOORS["go"].entry).unlink()


def nothing_walks_through(root: pathlib.Path) -> None:
    """The door is declared and no caller uses it — a parameter, not a door."""
    tree(root)
    write(root / guard.DOORS["go"].walks, "conn, err := slate.Dial(head, identity)\n")


def no_reexport(root: pathlib.Path) -> None:
    tree(root)
    write(root / guard.REEXPORT[0], 'export { Client } from "./client.js";\n')


def rostered_client_gone(root: pathlib.Path) -> None:
    tree(root)
    (root / "clients" / "go" / "go.mod").unlink()


def no_clients_at_all(root: pathlib.Path) -> None:
    """The never-fires case: nothing found must not read as nothing wrong."""
    (root / "clients").mkdir()


CASES = [
    ("three clients with doors pass", clean, 0, "3 clients, each with a transport door"),
    ("a fourth client with no door fails and names it", fourth_client, 1, "clients/rust is a client and DOORS has no entry"),
    ("a directory with no manifest fails rather than being skipped", no_manifest, 1, "declares none of"),
    ("a removed door fails", door_removed, 1, "typescript's transport door is gone"),
    ("a renamed-away entry file fails", entry_file_gone, 1, "clients/go/slate/client.go does not exist"),
    ("a door nothing walks through fails", nothing_walks_through, 1, "nothing walks through go's transport door"),
    ("a dropped grpc-js re-export fails", no_reexport, 1, "no longer re-exports grpc-js"),
    ("a rostered client that is gone fails", rostered_client_gone, 1, "is not a client directory any more"),
    ("no clients at all fails", no_clients_at_all, 1, "no client directories found"),
]


def main() -> int:
    was = (guard.ROOT, guard.CLIENTS)
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
        guard.ROOT, guard.CLIENTS = was

    for failure in failures:
        print(failure, file=sys.stderr)
    print(f"{len(CASES) - len(failures)} passed, {len(failures)} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
