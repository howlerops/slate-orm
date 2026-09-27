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

#: A source that runs to completion, one that does not, and three that read
#: their own command line: through the shared parser, by hand with a refusal,
#: and by hand without one.
RUNS = "fn main() { println!(\"done\"); }\n"
SERVES = 'fn main() { println!("LISTENING 1"); std::future::pending::<()>(); }\n'
PICKY = 'fn main() { let _ = sections::from_args(&["rpc", "lease"]); }\n'

#: The `s3_server` shape: every argument matched, an unknown one exits 2.
#:
#: Spelled by hand rather than through `sections::from_args`, because that is
#: the half rule 5 exists for — an example parsing `std::env::args()` itself
#: was invisible to every rule when the roster was keyed on the shared parser.
HANDY = (
    'fn main() { for a in std::env::args().skip(1) { match a.as_str() {\n'
    '    "--json" => {}, _ => std::process::exit(2) } } }\n'
)

#: The defect rule 5 reports: read, compare, ignore anything else, exit 0.
#:
#: This is `bucket_layout` and `cost_calibration` as they were — `--jsonn`
#: printed the human listing and `--codl` ran the warm suite, each exiting 0.
SLOPPY = 'fn main() { let j = std::env::args().any(|a| a == "--json"); let _ = j; }\n'

#: A server that also reads its command line, which is `s3_server` exactly.
#: Rule 5 applies to it; rule 4 does not, because the runner never reaches the
#: refusal branch for an example it stops at the handshake.
SERVES_PICKY = (
    'fn main() { for a in std::env::args().skip(1) { match a.as_str() {\n'
    '    "--bucket" => {}, _ => std::process::exit(2) } }\n'
    '    println!("LISTENING 1"); std::future::pending::<()>(); }\n'
)


#: The run loop's two calls with a `continue` between them, which is what rule
#: 5 requires and what the real `run_examples.sh` does.
LOOP = """for name in $examples; do
    wanted=$(handshake "$crate" "$name")
    if [ -n "$wanted" ]; then
        continue
    fi
    bad=$(refuses "$crate" "$name")
done
"""


def script(
    floors: dict[str, int],
    shakes: dict[tuple[str, str], str],
    refusals: dict[tuple[str, str], str],
    loop: str | None = None,
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

    # The run loop, reduced to what rule 5 reads: the two calls and the
    # `continue` between them. Not a copy of the real loop — the guard looks at
    # control flow between two literal calls and nothing else — but it has to
    # be here, because without it every case would fail rule 5 instead of
    # saying what it is about. `loop` lets the three cases that *are* about
    # rule 5 replace it.
    return (
        (loop if loop is not None else LOOP)
        + "case \"$crate\" in\n"
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
    loop: str | None = None,
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
            # The guard's own two predicates, spelled out rather than
            # imported: a fixture that asked `check_examples_roster` which
            # examples need a line would agree with it by construction, and
            # every rule-4 case would pass against a broken pair.
            if ("std::env::args" in source or "sections::from_args" in source)
            and "future::pending" not in source
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
        runner.write_text(script(floors, shakes, refusals, loop))
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
        "`a/picky` reads its command line and has no line in `refuses()`",
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
        "a refusal for an example that reads no arguments is reported",
        {"a": {"one": RUNS, "picky": PICKY}},
        {("a", "picky"): "--not-a-section", ("a", "one"): "--not-a-section"},
        "names `a/one`, which reads no arguments",
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
        # The other never-fires half. Without this, deleting `READS_ARGS`
        # or renaming what it matches leaves rules 4 and 5 checking an empty
        # set and printing `ok`.
        "a tree where no example reads its command line is reported, not passed over",
        {"a": {"one": SERVES}},
        {("a", "one"): "--not-a-section"},
        "no example anywhere reads its command line",
        True,
    ),
    (
        # Rule 5's forward half, and the reason the roster key was widened:
        # this source was invisible to every rule when the key was
        # `sections::from_args`.
        "an example that reads arguments and refuses none is reported",
        {"a": {"sloppy": SLOPPY}},
        {("a", "sloppy"): "--not-an-option"},
        "`a/sloppy` reads its command line and has no path that refuses",
        False,
    ),
    (
        "a hand-written refusal counts, so an s3_server-shaped example is clean",
        {"a": {"handy": HANDY}},
        {("a", "handy"): "--not-an-option"},
        "",
        False,
    ),
    (
        # Without the roster half, `SLOPPY` would only ever be caught by rule
        # 5 — and rule 5 reads the source, so an example that gained a
        # `process::exit` for some unrelated reason would pass it while still
        # never being handed a bad argument by a run.
        "an example that reads arguments by hand still needs a refusal line",
        {"a": {"handy": HANDY}},
        {},
        "`a/handy` reads its command line and has no line in `refuses()`",
        False,
    ),
    (
        # The exclusion, forward: a server is run for its handshake and the
        # runner moves on, so demanding a line for it would demand a line
        # that is never read.
        "a server that reads arguments is not asked for a refusal line",
        {"a": {"serving": SERVES_PICKY}},
        {},
        "",
        False,
    ),
    (
        # And backwards. Before the exclusion this reported "reads no
        # arguments", which is false about `s3_server` and would send the
        # reader to the wrong file.
        "a refusal line for a server is reported as unreachable",
        {"a": {"serving": SERVES_PICKY}},
        {("a", "serving"): "--not-an-option"},
        "never returns and is run for its handshake instead",
        False,
    ),
    (
        # Rule 5 does apply to a server: `s3_server` refuses, and one that
        # stopped would be the same silent pass one command-line away.
        "a server that reads arguments and refuses none is still reported",
        {"a": {"serving": SERVES.replace(
            "fn main() {",
            'fn main() { let _ = std::env::args().any(|a| a == "--x");',
        )}},
        {},
        "`a/serving` reads its command line and has no path that refuses",
        False,
    ),
]


#: Rule 5's cases, which are about the run loop rather than about any table.
#:
#: Their own list for the reason `SECTION_CASES` has one: every other case
#: passes `loop=None` and would need a fifth element that says nothing.
#:
#: name, the loop text, the text the report must carry.
LOOP_CASES: list[tuple[str, str, str]] = [
    (
        "the real loop's shape passes",
        LOOP,
        "",
    ),
    (
        # The day rule 4's exclusion becomes wrong. A server would be handed a
        # bad argument, its (absent) roster line would matter, and this file
        # would still be reporting that such a line should be deleted.
        "a loop that reaches the refusal probe for a server is reported",
        LOOP.replace("        continue\n", "        :\n"),
        "no longer `continue`s before the refusal probe",
    ),
    (
        "a loop that asks for a refusal before a handshake is reported",
        """for name in $examples; do
    bad=$(refuses "$crate" "$name")
    wanted=$(handshake "$crate" "$name")
done
""",
        "asks `refuses` before `handshake`",
    ),
    (
        # Why the search is windowed rather than over the whole script. A
        # mutation reading the *whole* file for `continue` survived every case
        # here: the fixture's only `continue` was the one being removed, so
        # the two searches agreed, and so do they in the real runner today.
        # They stop agreeing the moment the loop grows a `continue` anywhere
        # else — a skip, the timeout branch — and then a whole-file search
        # reads "the server branch still continues" off an unrelated line.
        "a `continue` outside the server branch does not satisfy rule 5",
        """for name in $examples; do
    wanted=$(handshake "$crate" "$name")
    if [ -n "$wanted" ]; then
        :
    fi
    bad=$(refuses "$crate" "$name")
    if [ -z "$bad" ]; then
        continue
    fi
done
""",
        "no longer `continue`s before the refusal probe",
    ),
    (
        # The never-fires half. A loop rewritten so neither call is spelled
        # this way leaves rule 5 reading nothing and printing `ok`, which is
        # the same shape as the four halves above it.
        "a loop with neither call is reported, not passed over",
        "for name in $examples; do\n    :\n done\n",
        "no longer calls both",
    ),
]


def main() -> int:
    failed = 0
    ran = 0
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
        ran += 1
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected {wanted!r}, got {found}")

    for name, loop, wanted in LOOP_CASES:
        try:
            found = run({"a": {"one": RUNS}}, {"a": 1}, {}, ["a"], loop=loop)
        except Exception as raised:  # noqa: BLE001 - a crash is this case failing
            found = [f"raised {raised!r} instead of reporting a problem"]
        ok = (not found) if not wanted else any(wanted in one for one in found)
        failed += not ok
        ran += 1
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
        ran += 1
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected {wanted!r}, got {found}")

    # The real tree, last, so a failure here reads as "the tree drifted"
    # rather than as a broken test.
    found = check_examples_roster.problems()
    ok = not found
    failed += not ok
    ran += 1
    print(f"{'ok  ' if ok else 'FAIL'}  the real tree's four tables agree")
    if not ok:
        for one in found:
            print(f"      {one}")

    print()
    # Counted as the cases run, not summed from the lists. It was
    # `len(CASES) + len(SECTION_CASES) + 1`, and adding a third list left it
    # reporting 30 for a run of 34 — a summary smaller than the run, which
    # `scripts/mutate.py` reads as the score. A mutation restoring the sum
    # survived every case here, because nothing in a test file tests the test
    # file's own arithmetic. Deriving the number makes that mutation
    # impossible rather than caught, which is the better of the two.
    print(f"{ran - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
