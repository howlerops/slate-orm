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


#: Every synthetic tree gets the two files `NOT_AN_INSTALLER` names, each
#: carrying an example, because a tree without them is a tree with a stale skip
#: list and the guard now says so. Seeding them here rather than in each case
#: keeps the twelve cases below about what they were about; a case that wants
#: the rot rule to fire overrides the entry with `{name: ""}`, which is what the
#: last two do. This is the same reason `run` writes nothing else: the fixture
#: is the guard's precondition, not the thing under test.
SKIPPED = dict.fromkeys(guard.NOT_AN_INSTALLER, SHELL)


def run(files: dict[str, str | None]) -> list[str]:
    """`files` over the seeded tree; a `None` value means "and not this one"."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        for name, body in {**SKIPPED, **files}.items():
            if body is None:
                continue
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body, encoding="utf-8")
        return guard.problems(root)


def never_fires() -> list[str]:
    """The guard's never-fires reports, read off an empty tree, not listed here.

    A tree with nothing in it violates every precondition those halves guard,
    so it names all of them — which makes it the roster, and makes a half added
    later join without anyone remembering to. Listing them in a constant, here
    or in the guard, would have been the obvious answer and rots exactly the
    way the thing it is checking for rots.
    """
    return run({"README.md": "nothing here\n"})


#: name, the tree, the text the report must carry ("" means clean).
CASES: list[tuple[str, dict[str, str | None], str]] = [
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
        "scripts/gen.py runs a pinned `go install` (example.com/cmd/x@v1.2.3) "
        "and never sets `GOTOOLCHAIN`",
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
        # The mutation that found the real defect. `generate_proto.py` explains
        # its `GOTOOLCHAIN=auto` in twenty lines of comment above the
        # assignment, so deleting the assignment left the name in the file
        # eight times and the guard green. A mention is not a setting.
        "GOTOOLCHAIN named only in a comment is not a decision",
        {
            ".github/workflows/ci.yml": PINNED,
            "scripts/gen.py": INHERITS.replace(
                "subprocess.run(",
                "# We want GOTOOLCHAIN=auto here because the plugin is pinned\n"
                "# and declares a floor above the workflow's Go.\n"
                "subprocess.run(",
            ),
        },
        "scripts/gen.py runs a pinned `go install` (example.com/cmd/x@v1.2.3) "
        "and never sets `GOTOOLCHAIN`",
    ),
    (
        # The other half of the same finding: the pin and the invocation are
        # apart. `generate_proto.py` keeps its two versions in constants fifty
        # lines above `subprocess.run(["go", "install", package])`, so a
        # pattern wanting both in one match never saw the only real installer
        # in the repository — and the guard's roster was itself and this file.
        "a pin held in a constant far from the `go install` is still a pin",
        {
            ".github/workflows/ci.yml": PINNED,
            "scripts/gen.py": (
                'import subprocess\nPKG = "example.com/cmd/x@v1.2.3"\n\n\n'.join([""] * 8)
                + 'subprocess.run(["go", "install", PKG])\n'
            ),
        },
        "scripts/gen.py runs a pinned `go install` (example.com/cmd/x@v1.2.3) "
        "and never sets `GOTOOLCHAIN`",
    ),
    (
        # Comment-stripping alone would have caught the real defect, because
        # `generate_proto.py`'s mentions are all `#` lines. It would not catch
        # this: a docstring survives the strip, and "we leave GOTOOLCHAIN
        # alone" is the opposite of a decision. The two protections are not
        # redundant, and the mutation restoring the bare-name pattern survived
        # every other case here until this one was written.
        "GOTOOLCHAIN named only in a docstring is not a decision",
        {
            ".github/workflows/ci.yml": PINNED,
            "scripts/gen.py": (
                '"""Install the generators. We leave GOTOOLCHAIN alone."""\n' + INHERITS
            ),
        },
        "scripts/gen.py runs a pinned `go install` (example.com/cmd/x@v1.2.3) "
        "and never sets `GOTOOLCHAIN`",
    ),
    (
        # This guard and its own test carry the pattern's documentation and
        # its fixture, so they look like installers and are skipped by name.
        # Without the skip, a roster of exactly those two reads as a guard
        # doing its job — which is what it did, for two days.
        "the guard and its own test are not counted as installers",
        {
            ".github/workflows/ci.yml": PINNED,
            "scripts/gen.py": DECIDES,
            "scripts/check_toolchain_pins.py": INHERITS,
            "scripts/test_check_toolchain_pins.py": INHERITS,
        },
        "",
    ),
    (
        # The pin pattern was `[\\w.\\-/]+@v?[\\d][\\w.\\-+]*`, which reads any
        # `@v<digit>` token as a Go pin. A file whose only pinned string is an
        # action reference or an npm range then reads as a pinned Go installer,
        # and is required to decide `GOTOOLCHAIN` over a line with nothing to
        # do with Go. The refusal would name `actions/checkout@v5`, which is
        # the kind of message that teaches a reader to distrust the guard.
        # `scripts/real.py` is here so the never-fires half stays quiet: a tree
        # whose only installer is the one under test would report "nothing
        # runs a pinned go install" whichever way this case went, and the
        # case would pass for the wrong reason.
        "an unrelated `@v` token beside a `go install ./...` is not a pin",
        {
            ".github/workflows/ci.yml": PINNED,
            "scripts/real.py": DECIDES,
            "scripts/gen.py": (
                '# see actions/checkout@v5 and node@v22\nRANGE = "pg@v16"\n' + UNPINNED
            ),
        },
        "",
    ),
    (
        # And the other direction, which is the one that would matter if the
        # narrowing went too far: a real `go install` target is a domain, at
        # least one more path element, and a version. Both of this
        # repository's are `google.golang.org/...`, and a pattern that stopped
        # matching those would silently stop watching the only file the guard
        # is for.
        "a real module path is still a pin",
        {
            ".github/workflows/ci.yml": PINNED,
            "scripts/gen.py": (
                'subprocess.run(["go", "install", '
                '"google.golang.org/grpc/cmd/protoc-gen-go-grpc@v1.6.2"])\n'
            ),
        },
        "scripts/gen.py runs a pinned `go install` "
        "(google.golang.org/grpc/cmd/protoc-gen-go-grpc@v1.6.2) "
        "and never sets `GOTOOLCHAIN`",
    ),
    (
        # The path elements are optional: `go install example.com@v1.2.3`
        # installs a module whose root is itself a main package. Written
        # because a mutation relaxing `(?: /… )+` to `*` survived every other
        # case here, which said the `+` was over-narrow rather than untested —
        # a pattern that stops matching a real target silently stops watching
        # the file it is for.
        "a module path with no elements below the domain is still a pin",
        {
            ".github/workflows/ci.yml": PINNED,
            "scripts/gen.py": 'subprocess.run(["go", "install", "example.com@v1.2.3"])\n',
        },
        "scripts/gen.py runs a pinned `go install` (example.com@v1.2.3) "
        "and never sets `GOTOOLCHAIN`",
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
    (
        # The roster rot the caveat in
        # `ledger/2026-09-29-a-guard-whose-roster-was-itself.md` named: the
        # skip list was checked in neither direction, so a guard renamed away
        # left a name in a frozenset excusing nothing.
        "a skipped file that is gone is reported",
        {"scripts/check_toolchain_pins.py": None},
        "is not a readable file",
    ),
    (
        # The worse half, because the file is still there and reads as a live
        # exemption: the example has been edited out, so the skip now hides any
        # real installer the file grows.
        "a skipped file that no longer carries an example is reported",
        {"scripts/check_toolchain_pins.py": "# no installer here any more\n"},
        "no longer carries a `go install` example",
    ),
]


def main() -> int:
    failed = 0
    ran = 0
    # What each case asked for and what it got, so the checks after the loop
    # can ask which halves a case is *about* rather than which case names are
    # still spelled the way someone remembers.
    asked: list[tuple[str, list[str]]] = []
    for name, files, wanted in CASES:
        try:
            found = run(files)
        except Exception as raised:  # noqa: BLE001 - a crash is this case failing
            found = [f"raised {raised!r} instead of reporting a problem"]
        asked.append((wanted, found))
        ok = (not found) if not wanted else any(wanted in one for one in found)
        failed += not ok
        ran += 1
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected {wanted!r}, got {found}")

    # The never-fires halves are themselves a never-fires hazard, which is the
    # caveat in `ledger/2026-09-29-the-skip-list-that-excused-nothing.md`: two
    # cases above covered them and nothing held those cases to existing, while
    # the seeded fixture is a precondition every case inherits, so a case could
    # satisfy it wrongly and a half would stop firing with nothing saying so.
    # A case counts only if it both *targets* the half — its expectation is
    # text from that report and not from another — and observed it, so a case
    # edited into asking for something else stops counting rather than keeps
    # covering by name.
    # And the idiom's own hazard, which
    # `ledger/2026-09-30-the-never-fires-halves-are-a-never-fires-hazard.md`
    # recorded: a case's expectation is `in`-tested against the report, so an
    # expectation broadened to a common fragment still passes — demonstrated
    # there by a mutation that changed one to `go install` and left every
    # check green. What makes an expectation *about* its case is that it does
    # not also match a different case's report. Checked here rather than by
    # making the match exact, because the reports carry paths and counts a
    # case has no reason to restate.
    every: set[str] = {one for _, got in asked for one in got}
    for want, got in asked:
        if not want or not got:
            continue
        # A report this case never produced. Two cases legitimately observing
        # the same report is not the failure — each may be about a different
        # part of it — so the comparison is against the reports outside this
        # case's own, which is what "would also pass against something else"
        # means.
        elsewhere = sorted(one for one in every - set(got) if want in one)
        ok = not elsewhere
        failed += not ok
        ran += 1
        print(f"{'ok  ' if ok else 'FAIL'}  {want!r} is about one report and not another")
        if not ok:
            print(f"        it also matches: {elsewhere[0][:120]}")

    halves = never_fires()
    # The regress stops here. Deriving the roster from a tree rather than a
    # constant moves the hazard up one level: a `never_fires` that came back
    # empty would make every check below vacuously true, which is the failure
    # this whole block exists to catch. That it is non-empty is the one thing
    # about the roster statable without maintaining the count by hand.
    failed += not halves
    ran += 1
    print(f"{'ok  ' if halves else 'FAIL'}  an empty tree names at least one half")

    for text in halves:
        ok = any(want and want in text and text in got for want, got in asked)
        failed += not ok
        ran += 1
        half = text.split(",")[0]
        print(f"{'ok  ' if ok else 'FAIL'}  a case above is about {half!r}")
        if not ok:
            print("        no case expects that report, or none made it fire")

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
