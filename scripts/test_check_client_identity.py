#!/usr/bin/env python3
"""Tests for `check_client_identity.py`, over files this one writes.

Run against the real clients, a rule that had stopped checking would pass for
as long as they stayed correct — which is the failure mode the guard exists to
prevent, one level up.

Run directly: `python3 scripts/test_check_client_identity.py`.
"""

from __future__ import annotations

import contextlib
import io
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_client_identity

GO = """\
type Identity struct {
\tPrincipal string
\tTenant string
\tRoles []string
}
"""

TYPESCRIPT = """\
export interface Identity {
  readonly principal: string;
  readonly tenant?: string;
  readonly roles?: string[];
}
"""

CASES = [
    ("the three members in both clients pass", GO, TYPESCRIPT, 0, "6 members"),
    (
        "a fourth Go field fails and names it",
        # The shape finding 10 was, arriving in a client that does not have it
        # yet: a new metadata slot, which is where a credential goes.
        GO.replace("\tRoles []string\n", "\tRoles []string\n\tExtra map[string]string\n"),
        TYPESCRIPT,
        1,
        "`Identity` declares `Extra`",
    ),
    (
        "a fourth TypeScript member fails too",
        # Both clients, because covering one of two is the mistake this whole
        # rule is a response to.
        GO,
        TYPESCRIPT.replace(
            "  readonly roles?: string[];\n",
            "  readonly roles?: string[];\n  readonly extra?: Record<string, string>;\n",
        ),
        1,
        "`Identity` declares `extra`",
    ),
    (
        "a member that disappeared is reported",
        # A rule checking a shape that is gone passes while covering nothing.
        GO.replace("\tTenant string\n", ""),
        TYPESCRIPT,
        1,
        "no longer declares `Tenant`",
    ),
    (
        "a file with no Identity at all fails, rather than passing",
        "package slate\n\nfunc unrelated() {}\n",
        TYPESCRIPT,
        1,
        "no `Identity` declaration found for go",
    ),
    (
        "a TypeScript file with no Identity fails too",
        GO,
        "export interface Other {\n  readonly x: string;\n}\n",
        1,
        "no `Identity` declaration found for typescript",
    ),
    (
        "an unexported Go field is not a member",
        # Lowercase fields are package-private and never reach the wire; the
        # pattern requires a capital, and this pins that rather than leaving it
        # to the regex being read correctly.
        GO.replace("\tRoles []string\n", "\tRoles []string\n\tinternal int\n"),
        TYPESCRIPT,
        0,
        "6 members",
    ),
    (
        "a method on the struct is not a member",
        GO + "\nfunc (id Identity) apply(ctx context.Context) context.Context {\n\treturn ctx\n}\n",
        TYPESCRIPT,
        0,
        "6 members",
    ),
]


def run(go: str, typescript: str) -> tuple[int, str]:
    with tempfile.TemporaryDirectory() as directory:
        home = pathlib.Path(directory)
        (home / "client.go").write_text(go)
        (home / "client.ts").write_text(typescript)
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            code = check_client_identity.main(
                [str(home / "client.go"), str(home / "client.ts")]
            )
        return code, out.getvalue()


def main() -> int:
    failed = 0
    for name, go, typescript, expected, wanted in CASES:
        code, said = run(go, typescript)
        ok = code == expected and (not wanted or wanted in said)
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected exit {expected} and {wanted!r}, got {code}")
            for line in said.splitlines():
                print(f"      {line}")
    print(f"\n{len(CASES) - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
