#!/usr/bin/env python3
"""No caveat is `open`, and every `narrowed` one is named here with its reason.

# What this is for

On 2026-10-01 the live frame — `open` plus `narrowed` — reached **zero open
and two narrowed** for the first time since `docs/caveat-status.json` existed.
It had been 268 that morning and 762 when the tracker was built. Nothing stops
it climbing back, and a number that took three sweeps to reach is a number
that goes wrong quietly: each new entry writes three or four caveats, so the
frame refills at the rate work gets done unless every entry triages its own.

So this is a ratchet, and it is deliberately the strict kind.

# The two rules

**No verdict is `open`.** `open` means nobody has decided, and a repository
whose backlog is zero is one where that state does not survive a commit. The
other four verdicts are all still available and all say something: `closed`
if the work is done and there is a witness, `deliberate` if the boundary is
right and here is why, `moment` if the claim was about one run, `narrowed` if
part is answered and the rest is named. Writing one of those is the work this
rule refuses to let anybody skip; it is not a rule against having gaps, it is
a rule against having *undecided* ones.

**Every `narrowed` verdict is in `STILL_NARROWED`, with why it is still
live.** `narrowed` is the one live verdict that is legitimate — something is
genuinely half done — and the roster is what stops it becoming the place
`open` rows go to hide. One entry per caveat, naming what has to happen, in
the `EXEMPT_BECAUSE` idiom this repository already uses in
`check_closed_caveats.py` and `check_toolchain_pins.py`: a list you are forced
to edit is a list that stays true.

# What it does not do

It cannot tell a `deliberate` verdict with a real argument from one with a
sentence. `docs/labelled-verdicts.json` measures that rate from the other side
and `scripts/read_deliberate.py` is the tool for re-reading them; this only
refuses the verdict that admits to having no argument at all. A session that
wanted to dodge it could write `deliberate` with filler — and would have to
write the filler, which is the same cost that makes a wrong exemption
uncomfortable to grant everywhere else here.

Run it directly, or let `scripts/check.sh`: `python3 scripts/check_live_frame.py`.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
STATUS = ROOT / "docs" / "caveat-status.json"

#: Every `narrowed` caveat, by (entry, key prefix), and what is left.
#:
#: Keyed on a prefix because `caveats.py` truncates a key at sixty characters
#: and matching the truncation by hand is how a roster entry comes to match
#: nothing — the failure `ledger/2026-10-01-the-live-frame-driven-down.md`
#: records one level out. A prefix that matches two rows is refused below.
STILL_NARROWED: dict[tuple[str, str], str] = {
    (
        "2026-09-29-every-guard-now-has-one.md",
        "Nothing runs `run_mutations.py` on a schedule.",
    ): (
        "The workflow is on `main`, so the `schedule:` half can fire, and the roster has "
        "now been run end to end by hand — 31 suites, all clean. What is left is the "
        "scheduled run itself: the cron is Monday 07:11 UTC and has not come round, and "
        "`workflow_dispatch` needs a push-access token this session does not have "
        "(`POST /actions/workflows/mutations.yml/dispatches` returns 403). Closes when a "
        "run appears under the workflow."
    ),
    (
        "2026-09-29-the-mutations-run-again-now.md",
        "Nothing runs `run_mutations.py` on a schedule.",
    ): (
        "The same caveat written in two entries on the same day, and the same residual. "
        "Both close on the same event: the first run appearing under the workflow."
    ),
}


def rows() -> list[dict]:
    return json.loads(STATUS.read_text(encoding="utf-8"))["verdicts"]


def main() -> int:
    problems: list[str] = []
    verdicts = rows()

    # The never-fires case first, as every guard here does: an empty tracker
    # would satisfy both rules below and mean nothing.
    if not verdicts:
        # Named by the path as given, not relative to ROOT: the guard's own
        # suite points STATUS at a temporary tracker, and `relative_to` on one
        # raises — which would make the never-fires case itself crash.
        print(f"{STATUS} holds no verdicts, so this compared nothing.", file=sys.stderr)
        return 1

    still_open = [v for v in verdicts if v.get("verdict") == "open"]
    for v in still_open:
        problems.append(
            f"{v['entry']}: `{v['key']}` is still `open`.\n"
            "  The backlog has been zero since 2026-10-01 and `open` means undecided.\n"
            "  Write `closed` with a witness, `deliberate` with the argument, `moment`\n"
            "  if the claim was about one run, or `narrowed` with a row in\n"
            "  STILL_NARROWED saying what is left."
        )

    # A bijection, checked from both ends. One roster row per narrowed caveat
    # and one narrowed caveat per roster row — because the failure that looks
    # like success is a prefix short enough to cover two caveats, where the
    # roster appears complete and one caveat is wearing another's reason.
    narrowed = [v for v in verdicts if v.get("verdict") == "narrowed"]
    covers: dict[tuple[str, str], list[str]] = {key: [] for key in STILL_NARROWED}
    for v in narrowed:
        hits = [
            key for key in STILL_NARROWED
            if key[0] == v["entry"] and v["key"].startswith(key[1][: len(v["key"])])
        ]
        for key in hits:
            covers[key].append(v["key"])
        if not hits:
            problems.append(
                f"{v['entry']}: `{v['key']}` is `narrowed` and is not in STILL_NARROWED.\n"
                "  A narrowed caveat is the one live verdict that is legitimate, and the\n"
                "  roster is what stops it becoming where undecided rows hide. Add a row\n"
                "  saying what is left and what closes it."
            )
        elif len(hits) > 1:
            problems.append(
                f"{v['entry']}: `{v['key']}` matches {len(hits)} STILL_NARROWED rows.\n"
                "  Lengthen the prefixes so each names one caveat."
            )

    for key, hit in sorted(covers.items()):
        if not hit:
            problems.append(
                f"STILL_NARROWED names {key[0]}: `{key[1]}`, which is no longer\n"
                "  narrowed. Drop the row. A roster carrying an entry for a caveat that\n"
                "  closed is the rot this file exists to make loud."
            )
        elif len(hit) > 1:
            problems.append(
                f"STILL_NARROWED's row for {key[0]}: `{key[1]}` covers {len(hit)}\n"
                f"  narrowed caveats: {', '.join(repr(h) for h in hit)}.\n"
                "  One of them is wearing the other's reason while the roster looks\n"
                "  complete. Lengthen the prefix and give each its own row."
            )

    for key, reason in sorted(STILL_NARROWED.items()):
        if not reason.strip():
            problems.append(f"STILL_NARROWED names {key[0]} with no reason.")

    if problems:
        print("the live caveat frame has grown:\n", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        print(f"\n{len(problems)} problem(s)", file=sys.stderr)
        return 1

    print(
        f"ok    {len(verdicts)} caveats, 0 open and {len(narrowed)} narrowed, "
        f"each named in STILL_NARROWED"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
