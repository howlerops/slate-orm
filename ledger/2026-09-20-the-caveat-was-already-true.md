# The guard's own "covers one file" caveat was describing a real miss

- **Date:** 2026-09-20
- **Author:** Claude, checking whether the caveat written an hour earlier was hypothetical
- **Touches:** `scripts/check_handlers.py`, `scripts/test_check_handlers.py`
- **Kind:** fix

## What changed

`check_handlers.py` reads every `.rs` under `slate-server/src` and
`slate-serverd/src` rather than `service.rs` alone, fails if it finds no
`fingerprint::check` at all, and gains a second exemption list for checks whose
table arrives already authorised. Its tests go 5 to 9.

## Why

The entry for that guard, written an hour before this one, said:

> **It covers one file.** `service.rs` is where the handlers are, and the check
> hard-codes it. A second service, or handlers moved to a module, would be
> outside it silently.

That was offered as a limitation to watch. It was already a miss:
`convert.rs:1451` has a `fingerprint::check` the guard never read. One `grep`
across the crate found it in about ten seconds, which is the part worth
recording — **the caveat was cheaper to check than to write**, and I wrote it
instead.

The call turns out to be safe, and safe for an interesting reason.
`query_from_proto_at` takes a `&TableDef` it did not resolve, so it *cannot*
authorise: it has no name to look up and no context to look it up for. Its
callers are `query`, `explain` and the join and chain handlers — which did not
authorise before converting until finding 8 was fixed the second time, three
commits ago. So this fingerprint was, until this afternoon, reachable by a
caller with no grant, and the guard written to catch that class could not see
it.

That makes it exactly the case the first guard's other caveat named — "a
handler can hold a `&TableDef` from one of the accounted sites and pass it
along" — so it is listed in `FINGERPRINT_BY_CALLER` with the callers that
check it, rather than skipped by shape. "Takes a table, so somebody else
checked" is the reasoning that was wrong three times today; written down, it
has to be re-argued when a caller is added.

The second change is smaller and is this repository's oldest lesson. A check
that scans a tree and finds nothing prints the same thing as a check that found
nothing wrong. If the handlers move, `SOURCES` goes stale and the guard turns
green over an empty walk. It now fails instead, naming both possibilities,
because `CLAUDE.md`'s "a check that never fires is a check nobody has debugged"
applies to a check that *stops* firing just as much.

## Alternatives rejected

**Scan the whole workspace.** Every crate, no `SOURCES` to go stale. It would
read `slate-kernel`, where `fingerprint` does not exist and `self.table(..)`
means something else entirely, and each false positive would need an exemption
whose reason is "different `table`". The two server crates are where the wire
handlers are, and the never-fires check is what makes the narrower scope safe
to state.

**Infer the by-caller case from the signature** — a function taking
`&TableDef` rather than a name cannot authorise, so exempt it automatically.
Tempting and wrong in the same way twice over: it would have exempted
`query_from_proto_at` silently through the whole period when its callers did
*not* authorise, and it encodes as a rule the inference that produced three
defects today.

**Fold this into the previous commit by amending it.** It is one commit old and
unpushed at the time I found the miss. Rejected for the reason
`2026-09-20-four-say-absent-one-says-present.md` gives about a different edit:
the sweep is separate work with its own result, and an entry that grows new
evidence after the fact stops being a record of what was known when. The
previous entry's caveat was honest and useful; it should stay, and this should
say it was already true.

**Leave it and note the miss.** What the previous entry effectively did. The
whole argument of that entry was that writing an invariant down beats
remembering it, so declining to extend the guard by ten lines on the day its
stated gap turned out to be occupied would be hard to defend.

## Evidence

**The miss, found by the grep the caveat should have prompted:**

```
$ grep -rln "fingerprint::check(\|self\.table(" crates/slate-server/src crates/slate-serverd/src
crates/slate-server/src/convert.rs
crates/slate-server/src/service.rs
```

Before: `2 bare resolutions, 13 fingerprint checks` — one file.
After: `28 files, 2 bare resolutions all accounted for, 14 fingerprint checks
all authorised first`.

**Four mutations. Two survived the first run**, and both were real missing
tests rather than redundancy:

```
!!  a stale by-caller entry goes unreported: SURVIVED
!!  only one source directory is scanned:    SURVIVED
```

The first because the stale-entry test only ever checked `UNAUTHORIZED`; the
second because **every test passed a single file path**, so the directory walk
— the entire point of the change — had no coverage at all. A guard widened to
read a tree, tested only against one file. Both have cases now:

```
ok  the never-fires guard is removed       -> a file with no fingerprint check at all fails, rather than passing
ok  the by-caller exemption is ignored     -> a handler that authorises before fingerprinting passes, ...
ok  a stale by-caller entry goes unreported-> a stale FINGERPRINT_BY_CALLER entry is reported too
ok  only one source directory is scanned   -> every file in the directory is read, not only the first
```

The directory case uses two files named `a_first.rs` and `z_last.rs` and puts
the defect in the second, so a walk that reads one file fails it whichever
order the filesystem returns.

`scripts/test_check_handlers.py` 9 passed; `scripts/check.sh` 25/25;
`ruff` and `ty` clean.

## What this does not do

**The by-caller list is a claim about callers that nothing checks.**
`FINGERPRINT_BY_CALLER` says `query_from_proto_at`'s callers authorise first.
Rule 2 holds `query` and `explain` to it because they fingerprint too — but a
caller that only converts, never fingerprinting, would satisfy nothing here.
The join and chain handlers are named in the entry and were read, not checked.

**Four lines is still the reach, still calibrated rather than derived**, and
now applied across 28 files rather than one. Nothing in the widening makes that
threshold better justified; it makes it apply in more places.

**Two crates, named explicitly.** A third server crate is outside this until
somebody adds it, and the never-fires check will not notice — it fires on
"nothing found anywhere", not on "a tree nobody listed". That is a weaker
guarantee than it sounds and I have not closed it.

**I did not re-audit the rest of `convert.rs`.** One `fingerprint::check` was
the grep's answer; whether that file does anything else schema-dependent before
its callers authorise, I did not look. The same question applies to every file
the widened walk now reads and passes.
