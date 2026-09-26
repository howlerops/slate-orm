#!/usr/bin/env python3
"""Tests for `check_examples_roster.py`, over a tree this one writes.

Run only against the real tree, every rule here would pass for as long as the
tree stayed correct — which is the whole failure this guard exists to prevent,
one level up, and which `check_demo_surface.py` demonstrated by having two of
its rules survive a mutation for exactly that reason.

So every case builds a small workspace: a `Cargo.toml` with members, an
`examples/` directory or two, and a `run_examples.sh` carrying the floors and
handshakes the case is about. The real tree is checked last, as the case that
says "and the tree agrees".

Run directly: `python3 scripts/test_check_examples_roster.py`.
"""

from __future__ import annotations

import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_examples_roster

#: A source that runs to completion, one that does not, and one that reads
#: section names off its command line.
RUNS = "fn main() { println!(\"done\"); }\n"
SERVES = 'fn main() { println!("LISTENING 1"); std::future::pending::<()>(); }\n'
PICKY = 'fn main() { let _ = sections::from_args(&["rpc", "lease"]); }\n'


def script(
    floors: dict[str, int],
    shakes: dict[tuple[str, str], str],
    refusals: dict[tuple[str, str], str],
) -> str:
    """A `run_examples.sh` with only the three tables the guard reads.

    Not a copy of the real script: the guard reads three `case` arms out of it
    and nothing else, and a fixture carrying the rest would go stale against
    the original for no benefit.

    The two per-example tables are wrapped in their real function names, and
    that is load-bearing rather than cosmetic: their arms are spelled
    identically — `crate/name) echo "…" ;;` — so the guard tells them apart by
    which function's body they sit in, and a fixture emitting bare `case`
    blocks would be checking a parse the real script never gets.
    """

    def table(name: str, arms: dict[tuple[str, str], str]) -> str:
        return (
            f"{name}() {{\n"
            + '    case "$1/$2" in\n'
            + "".join(
                f'        {crate}/{example}) echo "{line}" ;;\n'
                for (crate, example), line in arms.items()
            )
            + "    esac\n}\n"
        )

    return (
        "case \"$crate\" in\n"
        + "".join(f"    {name}) least={least} ;;\n" for name, least in floors.items())
        + "esac\n"
        + table("handshake", shakes)
        + table("refuses", refusals)
    )


#: A consistent crate every case sits on unless it is about its absence.
#:
#: The three never-fires guards return early, deliberately: with no floors
#: parsed, *every* crate is "missing a floor" and the real message is buried
#: under the noise. That is right for the guard and inconvenient for a
#: fixture, which would otherwise have to be about two rules at once — so
#: each case gets one consistent crate for free, and the three cases that are
#: about a table being empty pass `bare=True`.
SPARE = ("spare", "up")

#: The same, for rule 4: an example that takes sections, so that the
#: never-fires half of the refusal table is satisfied in every case that is
#: not about it.
SPARE_PICKY = ("spare", "choosy")


def run(
    crates: dict[str, dict[str, str]],
    floors: dict[str, int],
    shakes: dict[tuple[str, str], str],
    members: list[str] | None = None,
    bare: bool = False,
    refusals: dict[tuple[str, str], str] | None = None,
) -> list[str]:
    """Run the real guard over a workspace this writes.

    `refusals=None` means "a correct line for every example that takes
    sections", which is what the cases that are about the other three rules
    want: rule 4 then reports nothing and they stay about what they are about.
    A case that *is* about rule 4 passes the table it means.
    """
    if refusals is None:
        refusals = {
            (crate, name): "--not-a-section"
            for crate, examples in crates.items()
            for name, source in examples.items()
            if "sections::from_args" in source
        }
    if not bare:
        crates = {**crates, SPARE[0]: {SPARE[1]: SERVES, SPARE_PICKY[1]: PICKY}}
        floors = {**floors, SPARE[0]: 2}
        shakes = {**shakes, SPARE: "LISTENING"}
        refusals = {**refusals, SPARE_PICKY: "--not-a-section"}
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        for crate, examples in crates.items():
            where = root / "crates" / crate / "examples"
            where.mkdir(parents=True)
            for name, body in examples.items():
                (where / f"{name}.rs").write_text(body)
        # The spare is always a member; a case's own list names only its own
        # crates, since it does not know the spare exists.
        listed = list(crates) if members is None else [*members, SPARE[0]]
        listed = [name for name in dict.fromkeys(listed) if name in crates]
        manifest = root / "Cargo.toml"
        manifest.write_text(
            "[workspace]\nmembers = [\n"
            + "".join(f'    "crates/{name}",\n' for name in listed)
            + "]\n"
        )
        runner = root / "run_examples.sh"
        runner.write_text(script(floors, shakes, refusals))
        return check_examples_roster.problems(runner, root, manifest)


#: name, crates, floors, handshakes, the text the report must carry, and
#: whether to run without the spare crate above.
#: An empty expectation means the tree is consistent and nothing is reported.
CASES: list[tuple[str, dict, dict, dict, str, bool]] = [
    (
        "a crate whose floor matches, with no servers, is clean",
        {"a": {"one": RUNS}, "b": {"two": SERVES}},
        {"a": 1, "b": 1},
        {("b", "two"): "LISTENING"},
        "",
        False,
    ),
    (
        # The rule that found the real defect: two of four crates with
        # examples were absent from the script entirely.
        "a crate with examples and no floor is reported",
        {"a": {"one": RUNS}, "b": {"two": SERVES}},
        {"b": 1},
        {("b", "two"): "LISTENING"},
        "`a` has 1 example(s) and no floor",
        False,
    ),
    (
        "and the report says what line to add",
        {"a": {"one": RUNS, "three": RUNS}},
        {},
        {},
        "Add `a) least=2 ;;`",
        False,
    ),
    (
        # A floor *below* the count is the quiet one: `-lt` still passes, so
        # deleting a benchmark stops being caught and nothing says so.
        "a floor below the real count is reported",
        {"a": {"one": RUNS, "three": RUNS}},
        {"a": 1},
        {},
        "has 2 example(s) and a floor of 1",
        False,
    ),
    (
        "a floor above the real count is reported too",
        {"a": {"one": RUNS}},
        {"a": 9},
        {},
        "has 1 example(s) and a floor of 9",
        False,
    ),
    (
        "a floor for a crate with no examples is reported",
        {"a": {"one": RUNS}},
        {"a": 1, "gone": 3},
        {},
        "carries a floor for `gone`, which has no examples",
        False,
    ),
    (
        # The hang. This is what the first run of `run_examples.sh` did.
        "an example that never returns and has no handshake is reported",
        {"a": {"one": RUNS, "server": SERVES}},
        {"a": 2},
        {},
        "`a/server` never returns and has no handshake line",
        False,
    ),
    (
        "a handshake for an example that is gone is reported",
        {"a": {"one": RUNS, "server": SERVES}},
        {"a": 2},
        {("a", "server"): "LISTENING", ("a", "deleted"): "UP"},
        "names `a/deleted`, which is not an example here any more",
        False,
    ),
    (
        "a handshake for an example that exits is reported",
        {"a": {"one": RUNS, "server": SERVES}},
        {"a": 2},
        {("a", "server"): "LISTENING", ("a", "one"): "UP"},
        "names `a/one`, which runs to completion",
        False,
    ),
    (
        # The budget-spending one: the binary is up and printing, and the
        # runner waits thirty seconds for a line it never says.
        "a handshake waiting for a line the source never prints is reported",
        {"a": {"server": SERVES}},
        {"a": 1},
        {("a", "server"): "READY"},
        "waits for `READY` from `a/server`, which does not print it",
        False,
    ),
    (
        # A directory under `crates/` that is not a workspace member is built
        # by nothing, so demanding a floor for it would be wrong.
        "examples in a directory that is not a member are not demanded",
        {"a": {"one": RUNS}, "detached": {"two": RUNS}},
        {"a": 1},
        {},
        "",
        False,
    ),
    (
        "no floors at all is reported as the table having moved",
        {"a": {"one": SERVES}},
        {},
        {("a", "one"): "LISTENING"},
        "The `case` was rewritten",
        True,
    ),
    (
        "no handshakes at all is reported, not passed over",
        {"a": {"one": SERVES}},
        {"a": 1},
        {},
        "delete this guard deliberately",
        True,
    ),
    (
        # `bare`, because the spare crate above is a server and would satisfy
        # this guard for every other case. A mutation survived until this
        # case existed: with the spare always present, deleting the guard
        # changed no verdict.
        "a tree where nothing awaits forever is reported, not passed over",
        {"a": {"one": RUNS}},
        {"a": 1},
        {("a", "one"): "UP"},
        "no example anywhere awaits forever",
        True,
    ),
    (
        "a workspace with no examples anywhere is reported",
        {},
        {"a": 1},
        {("a", "one"): "LISTENING"},
        "no workspace member has an `examples/` directory",
        True,
    ),
]


#: Rule 4's cases, which are about the refusal table and nothing else.
#:
#: A shorter shape than `CASES` on purpose: the floors and the handshake table
#: are derived from the crates here, because a rule-4 case that also had to
#: state a floor would be two rules' fixture and would go red for the wrong
#: reason when the other rules changed — which is the defect `_least()` above
#: exists for, one level down.
#:
#: name, crates, the refusal table (`None` for a correct one), the text the
#: report must carry, and whether to run without the spare crate.
SECTION_CASES: list[tuple[str, dict, dict | None, str, bool]] = [
    (
        "an example that takes sections and has a refusal line is clean",
        {"a": {"picky": PICKY}},
        None,
        "",
        False,
    ),
    (
        # The defect, back again: `head_report --typo` printed its header, ran
        # nothing and exited 0. An example that gains sections without a
        # refusal line is in exactly that state, and the run stays green.
        "an example that takes sections and has no refusal line is reported",
        {"a": {"picky": PICKY}},
        {},
        "`a/picky` takes section names and has no line in `refuses()`",
        False,
    ),
    (
        "a refusal for an example that is gone is reported",
        {"a": {"picky": PICKY}},
        {("a", "picky"): "--not-a-section", ("a", "ghost"): "--not-a-section"},
        "`refuses()` names `a/ghost`, which is not an example here any more",
        False,
    ),
    (
        "a refusal for an example that takes no sections is reported",
        {"a": {"one": RUNS, "picky": PICKY}},
        {("a", "picky"): "--not-a-section", ("a", "one"): "--not-a-section"},
        "names `a/one`, which takes no sections",
        False,
    ),
    (
        # The one that reads as its own opposite. `rpc` is a section this
        # source has, so the example runs, exits 0, and the runner reports
        # `wanted 2` — which sends the next reader looking for a refusal that
        # never went anywhere.
        "a refusal argument the example's own source carries is reported",
        {"a": {"picky": PICKY}},
        {("a", "picky"): "rpc"},
        "which is a string in its own source",
        False,
    ),
    (
        # Not about rule 4 at all: it is about `body()`, which slices a
        # function out of the script before either per-example table is
        # parsed. Both tables' arms are brace-free in the real tree, so a
        # closing-brace pattern without its `^` anchor slices exactly the same
        # text and every other case here passes — a mutation dropping the
        # anchor survived, which is how this case came to exist. An arm
        # carrying a `${…}` ends the body early and silently drops every arm
        # after it, including the spare's, so the guard reads half a table and
        # reports `ok`.
        "an arm containing a brace does not truncate the table",
        {"a": {"picky": PICKY}},
        {("a", "picky"): "--${nope}"},
        "",
        False,
    ),
    (
        "no refusals at all is reported, not passed over",
        {"a": {"one": SERVES}},
        {},
        "nothing hands a built example a section name it does not have",
        True,
    ),
    (
        # The other never-fires half. Without this, deleting `TAKES_SECTIONS`
        # or renaming what it matches leaves rule 4 checking an empty set and
        # printing `ok`.
        "a tree where no example takes sections is reported, not passed over",
        {"a": {"one": SERVES}},
        {("a", "one"): "--not-a-section"},
        "no example anywhere reads section names",
        True,
    ),
]


def main() -> int:
    failed = 0
    for name, crates, floors, shakes, wanted, bare in CASES:
        # `detached` is the one case about a directory that is not a member,
        # so it is the one case that cannot list every crate it wrote.
        members = [name for name in crates if name != "detached"]
        try:
            found = run(crates, floors, shakes, members, bare=bare)
        except Exception as raised:  # noqa: BLE001 - a crash is this case failing
            found = [f"raised {raised!r} instead of reporting a problem"]
        ok = (not found) if not wanted else any(wanted in one for one in found)
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected {wanted!r}, got {found}")

    for name, crates, refusals, wanted, bare in SECTION_CASES:
        floors = {crate: len(examples) for crate, examples in crates.items()}
        shakes = {
            (crate, example): "LISTENING"
            for crate, examples in crates.items()
            for example, source in examples.items()
            if "future::pending" in source
        }
        try:
            found = run(crates, floors, shakes, refusals=refusals, bare=bare)
        except Exception as raised:  # noqa: BLE001 - a crash is this case failing
            found = [f"raised {raised!r} instead of reporting a problem"]
        ok = (not found) if not wanted else any(wanted in one for one in found)
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected {wanted!r}, got {found}")

    # The real tree, last, so a failure here reads as "the tree drifted"
    # rather than as a broken test.
    found = check_examples_roster.problems()
    ok = not found
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  the real tree's four tables agree")
    if not ok:
        for one in found:
            print(f"      {one}")

    print()
    total = len(CASES) + len(SECTION_CASES) + 1
    print(f"{total - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
