# One port allocator instead of two, and a `max` that had never mattered

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `scripts/free_ports.py`, `scripts/test_free_ports.py`,
  `examples/explorer/run.sh`, `examples/batchbench/run.sh`
- **Kind:** refactor

## What changed

The two `PORTS` heredocs are one file. `scripts/free_ports.py` offers
`--count N` free ports below the ephemeral range; `examples/explorer/run.sh`
asks for five and `examples/batchbench/run.sh` for one, each in a single line,
both resolving it through the `$root` they already compute.

Putting the copies side by side showed something neither had on its own. Both
wrote the clamp as

```python
high = max(ephemeral_low - 1, 10100)
candidate = random.randint(10000, min(high, ephemeral_low - 1))
```

and `min(max(x, 10100), x)` is `x`. **The `max` never changed the answer**, at
any floor — checked over every value from 2 to 69999, no exceptions. The clamp
is now `draw(LOW, below - 1)` and nothing else.

`scripts/test_free_ports.py` calls the function rather than `exec`ing a
heredoc, and grew from 18 cases to 26. Five of the new ones exist because a
mutation survived.

## Why

The entry that tested the heredocs asked for this and said what it cost not to
have it:

> **It does not remove the duplicate.** Two allocators with one behaviour is
> one edit away from two behaviours, and the guard makes that edit *visible*
> rather than impossible: somebody who changes one copy and not the other
> still gets green, because both copies independently satisfy the invariant.
> — `ledger/2026-09-28-the-port-allocator-runs-in-a-test-now.md`

Exactly right, and the thing it predicted is already visible in the diff: the
two copies had *drifted in shape* — the explorer one declared `low` and `high`
as names and loops until it has five distinct ports; the batchbench one inlined
both and loops until it has one, with no distinctness to enforce. Same
arithmetic, two spellings, and a reader comparing them has to do the algebra to
see that they agree. Nobody had, which is how a dead `max` survived in both.

Removing it is a behaviour-preserving change and was verified as one rather
than argued: `min(max(f - 1, 10100), f - 1) == f - 1` for every integer floor
from 2 to 69999, which is every floor a Linux `ip_local_port_range` can
plausibly carry and then some.

## Alternatives rejected

**Leave the `max` in.** It is one line, it is harmless, and removing it makes
this diff bigger than "remove the duplicate". Rejected because the previous
entry reasoned at length about a floor of 10050 as the *adversarial* one —
where the `max` wins and the `min` is therefore load-bearing — and that
reasoning is about an interaction between two terms, one of which does nothing.
Leaving provably dead code in a file whose whole subject is a subtle clamp
invites the next reader to reconstruct the same wrong model. It is also the
kind of thing this repository's standards call out directly: a comment
restating the code is noise, and dead code is worse, because it reads as a
requirement.

**A shared shell function instead of a Python file.** `run.sh` is bash and
sourcing a `lib.sh` would keep it in one language. Rejected: the allocator is
Python either way, so a shell wrapper adds a layer without removing one, and
the file being importable is what let the test drop `exec` on an extracted
heredoc — 26 cases against a function beats 18 against a string.

**Keep the test extracting the heredoc, pointed at the new file.** It would
have needed the least rewriting. Rejected because the extraction existed only
because there was nothing importable, and keeping it would mean the test still
could not reach `floor()` separately from `free()` — which is the split that
found two of the five missing cases.

**Take the count from the caller's own parsing instead of `--count`.** The
explorer runner reads five names out of one line; it could read however many
arrive. Rejected: the count is a thing the caller depends on, and an allocator
that quietly offered four would leave the fifth name empty rather than fail.
The test asserts the count for that reason, and asking for it explicitly is
what makes the assertion meaningful.

## Evidence

- `python3 scripts/test_free_ports.py`: **26 passed, 0 failed**, from 18.
- Both runners, against real ports rather than a fixture:
  - `python3 scripts/test_run_teardown.py` with a prebuilt `slate-serverd`:
    **1 passed, 0 failed** — the explorer runner started a head node and three
    adapters on allocated ports and reaped **7 processes** on `SIGTERM`.
  - `bash examples/batchbench/run.sh --rows 10 --runs 1`: exit 0, three clients
    reporting. The ratios (python 8.2, go 7.9, typescript 14.3) are one run at
    ten rows and are not a measurement of anything — what they show is that the
    head node bound the allocated port and all three clients reached it.
  - `bash -n` on both.
- The dead `max`, enumerated rather than argued: `min(max(f - 1, 10100), f - 1)`
  differs from `f - 1` for **0 of the 69,998 floors** from 2 to 69999.
- **Ten mutations, in two runs.** The first found four problems and every one
  of them was in the *test*:
  - `ledger/mutations/20260929T165126-scripts-free-ports-py.json` — five cases,
    one caught. `a floor below the bottom is caught rather than raised`:
    caught. The other four:
    - *the clamp offers the floor itself* **survived**: with a floor of 32768
      the bad port is hit about once in twenty thousand draws, so the property
      was being sampled rather than asserted.
    - *the floor is assumed rather than read* **survived**: every arithmetic
      case passes the floor in as a parameter and the two fallback cases want
      `ASSUMED`, so a `floor()` that never opened the file passed the suite.
      That is the exact defect the file exists to prevent.
    - *distinctness is dropped* **survived**, for the same sampling reason.
    - *a garbled range is no longer caught* scored **NOTHING RAN** — narrowing
      `except (OSError, ValueError)` to `OSError` made the test raise out of
      `main`, so no tally was printed. That is the second of `mutate.py`'s six
      documented lies, arriving through a test case with no guard of its own.
  - `ledger/mutations/20260929T165229-scripts-free-ports-py.json` — the same
    five (with the last replaced by a bottom-of-range mutation), **five of
    five caught**, after `free` took an injectable `draw`, `floor()` got a case with a readable
    file, and every `RANGE` case reported a failure rather than raising.
- `sh scripts/check.sh`: **85 passed, all of them**, exit 0.

## What this does not do

**It does not measure the thing the allocator is for.** Nothing here shows that
a port below the ephemeral range is *actually* safe from the kernel reusing it;
that is a claim about Linux, taken from the documented behaviour of
`ip_local_port_range` and from the CI failure that prompted it. Every case here
checks the arithmetic against a floor, not the kernel against the arithmetic.

**The probe is still a race.** `free()` binds each candidate, closes it, and
hands the number to a caller that binds it later. Nothing between those moments
stops a *deliberate* listener taking it — only the kernel's automatic
allocation is excluded. That was true of both heredocs and is unchanged; it is
why the original failure was rare rather than constant.

**`--count` has no upper bound.** Asking for more ports than the range holds
loops forever rather than failing, because the distinctness check rejects every
repeat and there is nothing left to draw. Five is the most anything asks for
and twenty thousand candidates is the smallest range in play, so it cannot
happen today; a caller asking for 30,000 would hang with no message.

**Nothing stops a third copy appearing.** `the runners call the allocator`
checks the two `run.sh` in `CALLERS` and that neither carries a `PORTS` heredoc
any more. A *new* harness writing its own allocator is not in that list and
nothing adds it — the same shape as the roster problem
`ledger/2026-09-29-the-proto-roster-named-one-copy-of-three.md` fixed by
walking a tree, and not fixed here.
