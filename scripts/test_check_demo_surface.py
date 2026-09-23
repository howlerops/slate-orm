#!/usr/bin/env python3
"""Tests for `check_demo_surface.py`, over files this one writes.

Run against the real adapter and the real UI, a check that had stopped checking
would pass for as long as those two stayed correct — which is the failure mode
the guard exists to prevent, one level up. Each case builds the smallest pair of
files that exhibits one rule.

The cases that must NOT fire matter as much as the ones that must: this guard's
whole justification is that it fails with a one-line remedy rather than on every
surface task, and a guard people switch off is worth less than no guard.

Run directly: `python3 scripts/test_check_demo_surface.py`.
"""

from __future__ import annotations

import contextlib
import io
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_demo_surface

#: Two endpoints the real roster leaves out on purpose, so a fixture can use
#: them without inventing names the staleness check would call stale.
LEFT_OUT = sorted(check_demo_surface.NOT_IN_THE_UI)[:2]

#: One the UI really does call.
REACHED = "/api/query"

#: The clause the summary adds when it has read the README.
README_SUMMARY = "the README's counts agree"

#: A problem nothing else produces, to prove `main` asks for the README's.
STAND_IN = "a stand-in problem, to prove main asks"


def run(
    ui_calls: list[str],
    extra: list[str] | None = None,
    drop: list[str] | None = None,
) -> tuple[int, str]:
    """Run the guard over an adapter and a UI this writes.

    The adapter serves **every endpoint on the real roster**, plus `REACHED`,
    plus anything a case adds. That is not padding: the staleness check reports
    a rostered endpoint the adapter does not serve, so a fixture serving three
    of them would fail eleven times on entries it was never testing — which is
    the trap `test_check_handlers.py` records under its own `PREAMBLE`, met
    again here on the first run.

    `drop` removes one, for the case that is *about* a stale entry.
    """
    served = sorted(
        (set(check_demo_surface.NOT_IN_THE_UI) | {REACHED} | set(extra or []))
        - set(drop or [])
    )
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        adapter = root / "adapter.py"
        adapter.write_text(
            "ROUTES = {\n"
            + "".join(f'    "{name}": "handler",\n' for name in served)
            + "}\n"
        )
        ui = root / "src"
        ui.mkdir()
        # Split across two files, first call in the first and the rest in the
        # second, because the walk is part of what is being tested: with every
        # call in `api.ts`, a mutation reading only the first file survived.
        # `sorted(rglob)` puts `api.ts` before `panels.tsx`, so a case with two
        # calls now fails if the second file is skipped.
        (ui / "api.ts").write_text(
            "".join(f'const a = call("{name}");\n' for name in ui_calls[:1])
            or "export const nothing = 1;\n"
        )
        (ui / "panels.tsx").write_text(
            "".join(f'const b = call("{name}");\n' for name in ui_calls[1:])
            or "export const also = 2;\n"
        )
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            code = check_demo_surface.main([str(adapter), str(ui)])
        return code, out.getvalue()


#: A seed, a policy and a README that agree, as the real three do.
#:
#: The cutoff year is one a book is published *exactly on*, which the real
#: seed has none of — and that absence is why `year > cutoff` survived a
#: mutation against the tree. A fixture can put a row on the boundary; the
#: tree cannot be asked to.
SEED = """\
func seed() {
\tbook(1, 1, "Before", 1959, 4.0),
\tbook(2, 1, "Exactly On", 1960, 4.1),
\tbook(3, 1, "After", 1961, 4.2),
}
"""
CONFIG = 'using = "year >= 1960"\n'
README = "- `reader` sees 2 of 3 books — a row policy hides everything\n  published before 1960, and no adapter is involved.\n"


def prose(
    seed: str = SEED, config: str = CONFIG, readme: str = README
) -> list[str]:
    """Run the real `readme_counts` over three files this writes."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        (root / "seed.go").write_text(seed)
        (root / "head.toml").write_text(config)
        (root / "README.md").write_text(readme)
        return check_demo_surface.readme_counts(
            root / "README.md", root / "seed.go", root / "head.toml"
        )


#: name, the three fixture files, and the text the one problem must contain.
#: An empty `wanted` means the three agree and nothing is reported.
PROSE_CASES: list[tuple[str, str, str, str, str]] = [
    (
        # This is also what catches `>=` written as `>`: the fixture has a
        # book published exactly on the cutoff, so under `>` the visible
        # count is 1, the README's correct `2 of 3` is reported as wrong, and
        # this case fails. The real seed has no book on the boundary, which
        # is why that mutation survived against the tree.
        "three files that agree report nothing",
        SEED,
        CONFIG,
        README,
        "",
    ),
    (
        # The survivor that started this. With the paths fixed, replacing this
        # whole function's body with `[]` changed no test's verdict.
        "a README naming a book count the seed does not have is reported",
        SEED,
        CONFIG,
        README.replace("2 of 3", "2 of 4"),
        "the seed has 3",
    ),
    (
        "a README naming a visible count the policy does not admit is reported",
        SEED,
        CONFIG,
        README.replace("2 of 3", "1 of 3"),
        "admits 2",
    ),
    (
        "a README naming a cutoff the policy does not use is reported",
        SEED,
        CONFIG,
        README.replace("before 1960", "before 1950"),
        "`head.toml` says `year >= 1960`",
    ),
    (
        "a seed with no book rows is reported, not counted as zero",
        "func seed() {}\n",
        CONFIG,
        README,
        "checked nothing",
    ),
    (
        "a policy spelled some other way is reported",
        SEED,
        'using = "year > 1959"\n',
        README,
        "spelled some other way",
    ),
    (
        "a README that no longer says `sees N of M` is reported",
        SEED,
        CONFIG,
        README.replace("sees 2 of 3 books", "sees two of the three books"),
        "delete this check deliberately",
    ),
    (
        "a README that no longer names a cutoff is reported",
        SEED,
        CONFIG,
        README.replace("published before 1960", "published too long ago"),
        "for the same reason",
    ),
]


#: name, the endpoints the UI calls, extra served, dropped, exit, wanted text.
CASES: list[tuple[str, list[str], list[str], list[str], int, str]] = [
    (
        "an endpoint the UI calls, with the rest rostered, passes",
        [REACHED],
        [],
        [],
        0,
        "1 of 13 adapter endpoints",
    ),
    (
        "a new endpoint in neither the UI nor the roster is reported by name",
        [REACHED],
        ["/api/brand-new"],
        [],
        1,
        "`/api/brand-new` is served by the adapters and called nowhere",
    ),
    (
        "and the report says what to do about it",
        [REACHED],
        ["/api/brand-new"],
        [],
        1,
        "add it to NOT_IN_THE_UI with one line",
    ),
    (
        "a rostered endpoint the adapters dropped is reported",
        [REACHED],
        [],
        [LEFT_OUT[0]],
        1,
        f"NOT_IN_THE_UI names `{LEFT_OUT[0]}`, which the adapters no longer serve",
    ),
    (
        "a rostered endpoint the UI started calling is reported",
        [REACHED, LEFT_OUT[0]],
        [],
        [],
        1,
        "has been made the other way",
    ),
    (
        "an adapter with no routes fails, rather than passing",
        # The never-fires half. `ROUTES` is a dict literal in one file; a
        # rewrite into anything else would leave the pattern matching nothing
        # and this printing `ok` over a tree it never looked at. Every rostered
        # entry goes stale in the same run and is reported too — unavoidable,
        # since "no routes" and "the roster names routes" are the same fact
        # twice, and the assertion below names the never-fires message, which
        # nothing else produces.
        [REACHED],
        [],
        [*sorted(check_demo_surface.NOT_IN_THE_UI), REACHED],
        1,
        # The route table's own words, not the shared "so this checked
        # nothing": both never-fires messages end that way, and asserting the
        # shared half let a mutation removing *this* guard pass on the other
        # one's message. Caught by the mutation run, which is the whole reason
        # the two messages say different things.
        "The route table moved",
    ),
    (
        "a UI that names no endpoint fails, rather than passing",
        # The same, for the other spelling, and it is the one more likely to
        # move: the UI could route every call through a helper taking a name
        # rather than a path.
        [],
        [],
        [],
        1,
        "names no adapter endpoint at all",
    ),
    (
        "an endpoint named only in the second UI file is still seen",
        # `sorted(rglob)` walks `api.ts` then `panels.tsx`; this case puts a
        # second call in the later file, so reading only the first misses it
        # and the roster entry goes unreported.
        [REACHED, LEFT_OUT[0]],
        [],
        [],
        1,
        "has been made the other way",
    ),
    (
        "a UI naming only an endpoint nothing serves reaches none",
        # `reached` is `called & served`, so an endpoint the UI names and the
        # adapters do not serve must not count — otherwise a typo in the UI
        # would satisfy the never-fires guard.
        ["/api/not-a-real-endpoint"],
        [],
        [],
        1,
        "names no adapter endpoint at all",
    ),
]


def main() -> int:
    failed = 0
    for name, ui_calls, extra, drop, expected, wanted in CASES:
        code, said = run(ui_calls, extra, drop)
        ok = code == expected and (not wanted or wanted in said)
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected exit {expected} and {wanted!r}, got {code}")
            for line in said.splitlines():
                print(f"      {line}")

    for name, seed, config, readme, wanted in PROSE_CASES:
        # A crash is a failure with a name, not a dead suite. Three of these
        # mutate a never-fires guard away, after which a `None` reaches the
        # narrowing `assert` below it and raises — and an unguarded call here
        # killed the run before it printed its summary, which `mutate.py`
        # could only report as "NOTHING RAN". That is the second of its five
        # lies: no test failed and no test ran look identical.
        try:
            found = prose(seed, config, readme)
        except Exception as raised:  # noqa: BLE001 - any crash is this failure
            found = [f"raised {raised!r} instead of reporting a problem"]
        ok = (not found) if not wanted else any(wanted in one for one in found)
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected {wanted!r}, got {found}")

    # `main` actually calls the README check, which none of the cases above
    # can see: they call `readme_counts` directly, and the real three files
    # agree, so deleting the call from `main` changed no verdict at all. A
    # surviving mutation said so. Standing in a problem nothing else produces
    # is the smallest thing that proves the wiring.
    out = io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
        code = check_demo_surface.main([], lambda: [STAND_IN])
    said = out.getvalue()
    ok = code == 1 and STAND_IN in said
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  a real run reports what the README check found")
    if not ok:
        print(f"        exit {code}, and the report did not carry it:\n      {said}")

    # The summary names the README half only when it ran, and the two halves
    # are checked together because either alone passes for the wrong reason: a
    # summary that always names it is a check reporting work it did not do
    # (`CLAUDE.md`'s "a skip is green", one level up), and a summary that never
    # names it hides the README check going away entirely.
    _, said = run([REACHED])
    ok = README_SUMMARY not in said
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  a fixture run does not claim the README")
    if not ok:
        print(f"      it ran over files that have no README:\n      {said}")

    # The guard against the real tree, which is the thing it is for. Kept last
    # so a failure here reads as "the tree drifted" rather than as a broken
    # test.
    out = io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
        code = check_demo_surface.main([])
    said = out.getvalue()
    ok = code == 0 and README_SUMMARY in said
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  the real adapter, UI and README agree")
    if not ok:
        for line in said.splitlines():
            print(f"      {line}")

    print()
    print(f"{len(CASES) + len(PROSE_CASES) + 3 - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
