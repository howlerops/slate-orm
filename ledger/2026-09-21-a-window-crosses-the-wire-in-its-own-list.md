# Windows on the wire, and the proto copy nothing was comparing

- **Date:** 2026-09-21
- **Author:** Claude Code, on `claude/rust-orm-record-layer-gswxlu`
- **Kind:** feature
- **Touches:** `slate-server` (`records.proto`, `convert.rs`, `service.rs`,
  `status.rs`, `tests/wire.rs`, new `tests/windows.rs`), the TypeScript
  client's proto copy, `slate-wasm/src/sql.rs`, `scripts/check_proto_copies.py`
  and its guard, `scripts/check.sh`, `.github/workflows/ci.yml`,
  `docs/orm-comparison.md`, `site/docs/roadmap.html`

## What changed

`Query.window` carries a `Window` message beside the aggregates, and the value
comes back in `Row.windowed` — a **third list**, not more `computed`.
`ColumnRef` gains a `windowed` arm that a `SortKey` may name and nothing else
may, which is SQL's own rule rather than a limitation. A window on a join
input, a grouped input or a keyset page is refused by name, as is a function
carrying a field it does not use. The daemon exposes `[limits]
max_window_rows`, and the four new kernel errors are classified rather than
falling to the wildcard.

Separately: **nothing was checking that the TypeScript client's copy of the
proto matched the canonical one.** `scripts/check_proto_copies.py` now does,
with its own tests, in `check.sh` and in CI.

## Why

The previous commit built the operator and left it reachable only from Rust,
which `docs/orm-comparison.md` recorded as `F2a`. A kernel feature no client
can ask for is the shape this repository closed as `CG1`, `G5` and `G8` — code
that compiles and nothing calls.

The proto guard is a different finding and a worse one. `clients/typescript`
keeps its own copy of `records.proto` because `@grpc/proto-loader` reads the
file at run time and an npm package cannot reach into a sibling crate. Two
files, one meaning, and **no check**. I found it by noticing they had diverged
after my own edit — which is exactly how it would have shipped: a field added
to one side only is a client building requests against a schema the server does
not have, and no type system sees it, because the loader reads the file it is
given.

## Alternatives rejected

**Fold window values into `Row.computed`.** One list instead of two, and it
reintroduces the arithmetic the split exists to remove: "the second computed
value" would mean a different ordinal depending on how many windows the query
asked for, and a client reading by index would be right until somebody added a
window. `JoinedRow` and `Group` were split for this reason already; this is the
third instance of the same argument.

**A `oneof` in `Window` instead of a function enum plus per-function fields.**
More precise in the type system and inconsistent with `Aggregate`, which is
flat and refuses a `COUNT(*)` carrying a column. Following the neighbour means
one refusal idiom rather than two, and the refusals are what a client actually
meets.

**Let a filter name a window.** It would have been one more arm in `resolve`
and it is wrong: a window is computed after `WHERE`. Postgres spells the thing
a filter-on-window wants as `QUALIFY` in some dialects and as a subquery in
standard SQL; either is a second plan, which is the `UNION` argument. Refused
with the reason in the message, so a caller learns what to write instead.

**Repeat the kernel's window rules in the converter** — the unordered rank, the
running distinct count, the zero offset. Faster refusals and a second copy that
can drift. `window_from_proto` builds the kernel's `Window` and maps its error
instead, so the rules are stated once.

**Split the row from the front twice** (columns, then computed, then windows).
The natural reading and wrong on a short row: `windows` is the length the
*request* fixed, so it is cut from the end, and whatever is missing comes out
of the middle section rather than silently relabelling a column as a window.

**Compare the proto copies semantically rather than byte for byte.** A parser
comparing only declarations would let the comments drift, and the comments in
that file carry most of the reasoning about what each field means. A client
author reading a stale one is the same failure one level up.

## Evidence

`sh scripts/check.sh`: 34 of 34 (the two new steps included). The
`slate-server`, `slate-serverd`, `slate-kernel` and `slate-wasm` suites pass
unfiltered. `tests/wire.rs` is 30 tests including a 600-case round trip that
now **generates** windows rather than pinning the field empty;
`tests/windows.rs` is 7 tests against a real server on a real socket.

The live oracle is the `Aggregate` RPC: a whole-partition window aggregate must
equal the grouped aggregate for that row's key, fetched through a different RPC
that folds rows away. Two operators sharing only the accumulator arithmetic,
compared across the wire, where a conversion that dropped a partition column
would leave the numbers plausible.

**Eight mutations on the converter, all caught:**

```
ok   the window list is dropped on the way out   ->  a_query_survives_the_round_trip
ok   the window list is dropped on the way in    ->  4 tests
ok   the sort is converted without the windows in scope -> the_query_can_order_by_a_window_value
ok   a window is nameable from a filter too      ->  a_filter_naming_a_window_is_refused
ok   window values are sent as columns           ->  3 tests
ok   a window on a join or grouped input is accepted -> a_window_on_a_join_input_is_refused
ok   a mismatched aggregate field is ignored     ->  a_mismatched_window_field_is_refused
ok   a mismatched offset is ignored              ->  a_mismatched_window_field_is_refused
```

Three mutations on the SQL front end's `OVER` refusal, also all caught,
including the one that matters most for the two-token check: matching the bare
word `over` rather than `over` followed by a paren breaks a column named
`over`, and `a_bare_word_over_is_left_alone` catches it.

**A repository guard caught what I forgot, again.**
`every_error_is_classified` failed on all four new `KernelError` variants at
once. `WindowTooLarge` is `ResourceExhausted` with the other memory guards; the
three specification refusals are `InvalidArgument`, because the remedy is a
different query rather than a smaller one.

**One recorded regression seed is not a defect.** `wire.proptest-regressions`
gained a line during the mutation run above — proptest shrank a deliberately
broken converter's failure and persisted it. It is kept, because replaying a
valid one-window query costs nothing and pins that shape even if the generator
changes, and it is annotated in place, because a saved regression reads as
"something was wrong here" and nothing was.

**And the SQL front end's refusal went stale inside one commit.** Written two
hours earlier, it said "the query spec this compiles to has no window, and
neither does the gRPC protocol the three clients speak" — true when written and
false by the end of the same piece of work. The message now names the one gap
that remains, and the test asserts the *new* wording rather than the substring
`protocol`, so the next person to close the other half has to come back here.

## What this does not do

**No client SDK has a window surface.** Python, Go and TypeScript can send one
only by building the proto message by hand, which is not an API. That is what
is left of `F2a`, and it is the part a user actually touches — this commit made
the feature *possible* to reach, not convenient. The conformance runner has no
window case for the same reason.

**The workbench still refuses `OVER`.** It runs the kernel directly in the
browser and could support windows with no wire at all; its `QuerySpec` has no
window field, and the headers, labels and result rendering all key off it.

**No window over a join or a chain**, on the wire or in the kernel. The wire
refuses one by name now, which is stronger than the kernel's position: there,
nothing refuses it because nothing can express it.

**The proto guard covers one pair of files.** `COPIES` is a dict so a second
pair costs a line, and nothing checks that the dict is *complete* — a third
copy added somewhere would be as unguarded as this one was. I looked for others
and found none, which is not the same as there being none.

**`Row.windowed` is not carried by `JoinedRow`.** A joined read computes no
windows, so there is nothing to carry, but that means a future window over a
join needs a protocol change and not just a kernel one. Worth knowing before
somebody plans the kernel half alone.
