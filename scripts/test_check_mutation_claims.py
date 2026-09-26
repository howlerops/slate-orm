#!/usr/bin/env python3
"""Cases for `check_mutation_claims.py`.

Every fixture builds its own ledger and its own records directory and passes
both explicitly. Nothing here reads the repository's own `ledger/`: that is the
failure #281 was about, and #288 repeated it four times in one task by adding a
parameter with a live default. The last case is a real run over the real tree,
which is the only place the repository's own entries are read.
"""

from __future__ import annotations

import io
import json
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_mutation_claims as guard

#: A record with every case caught.
CLEAN = {
    "at": "2026-09-23T10:00:00+00:00",
    "file": "scripts/x.py",
    "command": ["python3", "scripts/test_x.py"],
    "outcome": "clean",
    "cases": [{"name": "a case", "verdict": "caught", "caught_by": ["a test"]}],
}
#: The same run with one survivor, which is the discrepancy that matters.
WITH_SURVIVOR = {
    **CLEAN,
    "outcome": "problems",
    "cases": [
        {"name": "a case", "verdict": "caught", "caught_by": ["a test"]},
        {"name": "the awkward one", "verdict": "survived", "caught_by": []},
    ],
}
#: A survivor recorded as expected is still a survivor.
EXPECTED_SURVIVOR = {
    **CLEAN,
    "cases": [
        {"name": "known", "verdict": "survived-as-recorded", "caught_by": []},
    ],
}


def run(name: str, body: str, records: dict[str, dict] | None = None) -> tuple[int, list[str]]:
    """The guard over one entry and a records directory of its own."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        ledger = root / "ledger"
        ledger.mkdir()
        (ledger / name).write_text(body, encoding="utf-8")
        held = root / "mutations"
        held.mkdir()
        for filename, content in (records or {}).items():
            (held / filename).write_text(json.dumps(content), encoding="utf-8")
        return guard.check(ledger, held)


CASES: list[tuple[str, str, str, dict[str, dict] | None, int, int]] = [
    # name, entry filename, entry body, records, claims seen, problems
    (
        "an entry counting mutations with no citation is refused",
        "2026-09-23-a-slug.md",
        "Four mutations, and the tests caught them.\n",
        None,
        1,
        1,
    ),
    (
        "one citing a record that exists passes",
        "2026-09-23-a-slug.md",
        "Four mutations. See `ledger/mutations/r.json`.\n",
        {"r.json": CLEAN},
        1,
        0,
    ),
    (
        "a citation to a record that is not there is refused",
        "2026-09-23-a-slug.md",
        "Four mutations. See `ledger/mutations/gone.json`.\n",
        {"r.json": CLEAN},
        1,
        1,
    ),
    # The date scope, which is what makes this rule applicable at all: 241
    # entries predate records and cannot be made to comply.
    (
        "an entry from before the rule is exempt",
        "2026-09-21-a-slug.md",
        "Four mutations, and the tests caught them.\n",
        None,
        0,
        0,
    ),
    (
        "and the first day is inside the rule, not outside it",
        f"{guard.FIRST_DAY}-a-slug.md",
        "Four mutations, and the tests caught them.\n",
        None,
        1,
        1,
    ),
    # The three phrasings that slipped past the first version of `COUNT`, on
    # 2026-09-26, in one session. Each had run real mutations, each had a
    # record waiting in `ledger/mutations/`, and none cited one — the exact
    # failure this guard exists to catch, uncaught because the number sat on
    # the wrong side of the noun or there was no number in the sentence at all.
    (
        "a count after the noun is a claim",
        "2026-09-23-a-slug.md",
        "**Mutations**, six run, six caught.\n",
        None,
        1,
        1,
    ),
    (
        "an evidence table is a claim even with no count in the prose",
        "2026-09-23-a-slug.md",
        "A mutation, run twice, caught both times:\n\n"
        "| mutation | caught by |\n|---|---|\n| the sort is dropped | a test |\n",
        None,
        1,
        1,
    ),
    (
        "a table alone, with no sentence about mutations at all",
        "2026-09-23-a-slug.md",
        "| mutation | dialect | caught by |\n|---|---|---|\n| x | rust | y |\n",
        None,
        1,
        1,
    ),
    # And the other side of the widening: a table whose first column happens
    # to be about something else must not be read as a mutation table.
    (
        "a table about something else is not a mutation table",
        "2026-09-23-a-slug.md",
        "| caveat | verdict |\n|---|---|\n| a mutation would be nice | open |\n",
        None,
        0,
        0,
    ),
    # The `^` on the table arm, which is the only thing this case can tell.
    # An entry *describing* this guard quotes the header inline — as the entry
    # that widened the pattern does, in the sentence you are reading about.
    # Without the anchor that quotation is read as a claim to have run
    # mutations, and the entry is told to cite a run it never made. Written
    # after a mutation dropping the anchor survived every other case.
    (
        "an entry quoting the table header inline is not claiming a run",
        "2026-09-23-a-slug.md",
        "The evidence table this repository uses has the header "
        "`| mutation | caught by |`, which is the shape the guard looks for.\n",
        None,
        0,
        0,
    ),
    # A count is required. Without one, the ambient prose in every entry here
    # — policy statements about what a surviving mutation means — would each
    # demand a citation.
    (
        "prose about mutation without a count is not a claim",
        "2026-09-23-a-slug.md",
        "A surviving mutation is a missing test, which is why mutation testing matters.\n",
        None,
        0,
        0,
    ),
    # The teeth.
    (
        "claiming every mutation was caught beside a survivor is refused",
        "2026-09-23-a-slug.md",
        "Five mutations, all caught. See `ledger/mutations/r.json`.\n",
        {"r.json": WITH_SURVIVOR},
        1,
        1,
    ),
    (
        "an expected survivor still contradicts `all caught`",
        "2026-09-23-a-slug.md",
        "Five mutations, all caught. See `ledger/mutations/r.json`.\n",
        {"r.json": EXPECTED_SURVIVOR},
        1,
        1,
    ),
    (
        "an entry that does not claim a clean sweep is fine beside a survivor",
        "2026-09-23-a-slug.md",
        "Five mutations; one survived, and is recorded as such. See `ledger/mutations/r.json`.\n",
        {"r.json": WITH_SURVIVOR},
        1,
        0,
    ),
]


def main() -> int:
    ran = 0
    failed = 0
    for name, filename, body, records, want_seen, want_wrong in CASES:
        seen, wrong = run(filename, body, records)
        ok = seen == want_seen and len(wrong) == want_wrong
        ran += 1
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(
                f"        wanted {want_seen} claim(s) and {want_wrong} "
                f"problem(s), got {seen} and {wrong}"
            )

    # A malformed record must not crash the guard, and must not silently read
    # as "no survivors" either — it reads as unreadable, and the citation's
    # existence is still what was checked.
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        ledger = root / "ledger"
        ledger.mkdir()
        (ledger / "2026-09-23-a.md").write_text(
            "Four mutations, all caught. See `ledger/mutations/r.json`.\n"
        )
        held = root / "mutations"
        held.mkdir()
        (held / "r.json").write_text("{not json at all")
        try:
            seen, wrong = guard.check(ledger, held)
            ok = seen == 1 and not wrong
        except Exception as raised:  # noqa: BLE001 - a crash is this case failing
            ok = False
            wrong = [f"raised {raised!r}"]
    ran += 1
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  a malformed record does not crash the guard")
    if not ok:
        print(f"        got {wrong}")

    # And the real tree, which is what CI actually runs.
    out = io.StringIO()
    code = guard.main()
    ran += 1
    failed += code != 0
    print(f"{'ok  ' if code == 0 else 'FAIL'}  the real ledger's claims are all cited")
    if code != 0:
        print(f"        exit {code}: {out.getvalue()}")

    print()
    print(f"{ran - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
