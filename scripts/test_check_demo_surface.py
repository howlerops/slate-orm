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

    # The guard against the real tree, which is the thing it is for. Kept last
    # so a failure here reads as "the tree drifted" rather than as a broken
    # test.
    out = io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
        code = check_demo_surface.main([])
    ok = code == 0
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  the real adapter and the real UI agree")
    if not ok:
        for line in out.getvalue().splitlines():
            print(f"      {line}")

    print()
    print(f"{len(CASES) + 1 - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
