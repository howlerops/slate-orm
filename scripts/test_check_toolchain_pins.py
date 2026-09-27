#!/usr/bin/env python3
"""`check_toolchain_pins.py`, over trees this file writes.

Against written trees for the reason every guard here gives: run only against
`slate-orm`, each rule would pass for as long as the tree stayed correct, which
is also what a rule that does nothing does. The real tree is the last case.

Run directly: `python3 scripts/test_check_toolchain_pins.py`.
"""

from __future__ import annotations

import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_toolchain_pins as guard

#: A workflow that pins Go, and one that takes it from `go.mod`.
PINNED = """name: CI
jobs:
  go:
    steps:
      - uses: actions/setup-go@v6
        with:
          go-version: "1.24"
"""
FROM_MOD = """name: CI
jobs:
  go:
    steps:
      - uses: actions/setup-go@v6
        with:
          go-version-file: clients/go/go.mod
"""

#: An installer that decides, one that does not, and one installing nothing
#: pinned.
DECIDES = (
    'env = {**os.environ, "GOFLAGS": "-mod=mod", "GOTOOLCHAIN": "auto"}\n'
    'subprocess.run(["go", "install", "example.com/cmd/x@v1.2.3"], env=env)\n'
)
INHERITS = 'subprocess.run(["go", "install", "example.com/cmd/x@v1.2.3"])\n'
SHELL = "go install example.com/cmd/x@v1.2.3\n"
UNPINNED = 'subprocess.run(["go", "install", "./..."])\n'


def run(files: dict[str, str]) -> list[str]:
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        for name, body in files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body, encoding="utf-8")
        return guard.problems(root)


#: name, the tree, the text the report must carry ("" means clean).
CASES: list[tuple[str, dict[str, str], str]] = [
    (
        "a pinned workflow and an installer that decides is clean",
        {".github/workflows/ci.yml": PINNED, "scripts/gen.py": DECIDES},
        "",
    ),
    (
        # The defect: `setup-go@v6` changed what the default is, and a script
        # that never named the variable inherited the change.
        "a pinned workflow and an installer that inherits is reported",
        {".github/workflows/ci.yml": PINNED, "scripts/gen.py": INHERITS},
        "never mentions `GOTOOLCHAIN`",
    ),
    (
        # `local` is a decision too. The rule is about inheriting, not about
        # which value — a script whose pins must fit the workflow's Go is
        # coherent and says so.
        "setting GOTOOLCHAIN to local counts as deciding",
        {
            ".github/workflows/ci.yml": PINNED,
            "scripts/gen.py": INHERITS.replace(
                "subprocess.run(", 'os.environ["GOTOOLCHAIN"] = "local"\nsubprocess.run('
            ),
        },
        "",
    ),
    (
        "a shell `go install` is read, not only a Python argument list",
        {".github/workflows/ci.yml": PINNED, "scripts/gen.sh": SHELL},
        "scripts/gen.sh runs a pinned `go install`",
    ),
    (
        # `go install ./...` builds this module and cannot want a Go the
        # module does not declare, so it has no floor to collide with.
        "an unpinned `go install` is not a pinned one",
        {
            ".github/workflows/ci.yml": PINNED,
            "scripts/gen.py": DECIDES,
            "scripts/other.py": UNPINNED,
        },
        "",
    ),
    (
        # The never-fires half that matters second-most: with no pinned
        # `go-version` anywhere, `setup-go` does not force `GOTOOLCHAIN=local`
        # and the rule has nothing to rest on.
        "a tree whose workflows take Go from go.mod is reported, not passed over",
        {".github/workflows/ci.yml": FROM_MOD, "scripts/gen.py": INHERITS},
        "no workflow pins `go-version:`",
    ),
    (
        # And the one that matters most: if `GO_INSTALL` stops matching, every
        # installer is clean by vacuity.
        "a tree with no pinned `go install` at all is reported, not passed over",
        {".github/workflows/ci.yml": PINNED, "scripts/gen.py": UNPINNED},
        "nothing in this repository runs `go install",
    ),
    (
        "vendored trees are skipped, because their installers are not ours",
        {
            ".github/workflows/ci.yml": PINNED,
            "scripts/gen.py": DECIDES,
            "web/node_modules/p/setup.sh": SHELL,
        },
        "",
    ),
    (
        # Both never-fires halves fire together and the report says both,
        # rather than the first one masking the second.
        "an empty tree names both halves",
        {"README.md": "nothing here\n"},
        "no workflow pins `go-version:`",
    ),
]


def main() -> int:
    failed = 0
    ran = 0
    for name, files, wanted in CASES:
        try:
            found = run(files)
        except Exception as raised:  # noqa: BLE001 - a crash is this case failing
            found = [f"raised {raised!r} instead of reporting a problem"]
        ok = (not found) if not wanted else any(wanted in one for one in found)
        failed += not ok
        ran += 1
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected {wanted!r}, got {found}")

    # The real tree, last, so a failure here reads as "the tree drifted"
    # rather than as a broken test.
    found = guard.problems()
    ok = not found
    failed += not ok
    ran += 1
    print(f"{'ok  ' if ok else 'FAIL'}  this repository decides GOTOOLCHAIN where it must")
    for one in found:
        print(f"        {one}")

    # Counted, not summed. See the note in
    # `scripts/test_check_examples_roster.py`: a summed total drifted from its
    # run there and nothing noticed.
    print(f"\n{ran - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
