# The port allocator runs in a test now, at four different floors

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** `scripts/test_free_ports.py`, `scripts/check.sh`,
  `docs/caveat-status.json`, `scripts/check_closed_caveats.py`
- **Kind:** test

## What changed

`scripts/test_free_ports.py`: eighteen cases that extract the `PORTS` heredoc
from `examples/explorer/run.sh` and `examples/batchbench/run.sh`, execute it
with `open` patched so `/proc/sys/net/ipv4/ip_local_port_range` reports a floor
this file chooses, and assert every port it prints is at or above 10000 and
**below that floor**. Registered in `scripts/check.sh`, which now runs 72 steps.

This closes the caveat that asked for it:

> **Nothing tests the allocator itself.** There is no case asserting "every
> port is below the ephemeral floor", so a future edit could move it back into
> the range and only CI would notice, intermittently, which is where this
> started.
> — `ledger/2026-09-14-ports-below-the-ephemeral-range.md`

## Why

"Only CI would notice, intermittently" is the worst shape a defect has. The
first time, the demo job went red with `EADDRINUSE` on port 38721 and read as
the three SDKs disagreeing rather than as the harness handing out a port the
kernel was about to reuse for an outgoing connection.

The floors are the interesting part, and one of them earns its place by
catching a mutation nothing else caught. The allocator computes

```python
high = max(ephemeral_low - 1, 10100)
candidate = random.randint(low, min(high, ephemeral_low - 1))
```

The `min` is the safety and the `max` is what makes it necessary. At the Linux
default floor of 32768, `high` is 32767 and already below — so the `min` is a
no-op and a mutation deleting it changes nothing observable. Same at 15000. It
is only when the floor is low enough that `max(…, 10100)` wins — a floor of
10050 gives `high = 10100`, above the floor — that dropping the `min` starts
handing out ports inside the ephemeral range. So the adversarial floor is the
case that tests the safety, and the two realistic ones do not.

Two of the four floors exist for reasons the allocator's own comments assert
and nothing checked. "A container can be configured with a different one, and
picking below 32768 on a machine whose range starts at 15000 would reintroduce
exactly the bug" is a claim about a machine this container is not; patching
`open` makes that machine testable. So is the `except (OSError, ValueError)`
fallback, exercised by making the read raise.

The second copy is a finding of this work rather than something it fixes.
`examples/batchbench/run.sh` carries its own eleven-line allocator, written
from the explorer's — same `/proc` read, same fallback, same bounds, different
spelling, one port instead of five. The guard covers both, because the edit the
caveat fears applies to whichever copy somebody opens.

## Alternatives rejected

**Assert on the source text** — a regex requiring `ephemeral_low - 1` to still
appear. Cheap, needs no `exec`, and is the wrong check: it passes on

```python
candidate = random.randint(low, high)   # high was meant to be ephemeral_low - 1
```

which is the edit the caveat is actually about. The property is arithmetic, so
the arithmetic has to run.

**Extract the allocator into a shared Python module both `run.sh` files
import.** It is the right fix for the duplication and it is a larger change
than this one: `run.sh` is deliberately a shell script with no Python
dependency beyond `python3` itself, and a shared module under `scripts/` that both
shell out to adds a path they have to resolve, in two directories, for a
benchmark that currently needs nothing but the repository. Left as a finding
with the guard covering both copies, which is the cheap half of the benefit;
the expensive half can wait for somebody who wants to touch `run.sh` anyway.

**Bind real sockets at the real floor and assert nothing collides.** That is
the failure as it actually occurred, and it is unreproducible on purpose: the
collision needs an outgoing connection to be assigned the exact port in the
window between release and bind. A test that waits for that is a test that
passes for the wrong reason almost every time. Asserting the invariant that
makes the collision impossible is the falsifiable version.

**Leave `while True` in the batchbench copy alone** — noticed, not changed. If
nothing below the floor is free it spins forever rather than failing, which is
a different defect from this caveat's and would be fixed by a bounded retry.
Not touched here because the caveat is about *which* ports are offered, and
widening a test's subject to whatever else you noticed is how a small change
becomes unreviewable.

## Evidence

- `python3 scripts/test_free_ports.py`: 18 passed, 0 failed. Both `run.sh`
  copies, at floors 32768, 15000, 10050 and unreadable, plus the distinctness
  and count assertions and the below-the-bottom case.
- `python3 scripts/mutate.py`, record
  `ledger/mutations/20260928T215726-examples-explorer-run-sh.json`: four
  mutations, all caught by named cases.
  - Drawing from the ephemeral range — `randint(low, high)`, the original bug —
    caught **only** by the 10050 floor and the below-the-bottom case, for the
    reason argued above. The two realistic floors passed it, which is the
    measurement that justifies the adversarial one.
  - Assuming 32768 instead of reading `/proc`: caught by the 15000 and 10050
    floors.
  - Offering four ports instead of five: caught by all four count cases.
  - Allowing a duplicate: caught by the 10050 floor's distinctness case, whose
    narrow range makes a collision likely enough to see.
- `sh scripts/check.sh`: 72 passed, all of them.
- `python3 scripts/test_check_sh.py`: 4 passed — 111 steps, 6 blocks and 2 env
  vars, all accounted for.
- `python3 scripts/check_closed_caveats.py`: 373 closed, 348 witnessed, 25
  exempt.
- `python3 scripts/caveats.py`: 1573 caveats, 116 open, 63 narrowed, 373
  closed, 888 deliberate, 0 untriaged.

## What this does not do

**It does not run the allocator through `sh`.** The heredoc is extracted and
executed by Python, so a shell-level breakage — the heredoc marker renamed, the
function never called, `$(…)` capturing the wrong stream — is invisible to it.
The `the_heredoc_is_found` case catches the first of those and nothing catches
the other two; `examples/explorer/run.sh` is exercised end to end only by the
`--conformance` and `--e2e` jobs in CI.

**It does not remove the duplicate.** Two allocators with one behaviour is one
edit away from two behaviours, and the guard makes that edit *visible* rather
than impossible: somebody who changes one copy and not the other still gets
green, because both copies independently satisfy the invariant. What the guard
catches is a copy that stops being correct, not two copies that drift apart in
some way that is correct twice.

**The 10050 floor is chosen, not found.** It is the smallest floor I could see
by reading that makes `max(…, 10100)` bind, and I did not enumerate the range
of floors where the `min` matters. A floor between 10001 and 10100 would also
do it; whether anything interesting happens elsewhere in that window is
unexamined.
