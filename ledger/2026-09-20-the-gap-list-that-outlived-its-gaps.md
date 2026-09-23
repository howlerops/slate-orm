# The comparison doc called two shipped features its worst gaps

- **Date:** 2026-09-20
- **Author:** Claude, checking the product's own gap list rather than the security one
- **Touches:** `docs/orm-comparison.md`
- **Kind:** docs

## What changed

Three corrections to `docs/orm-comparison.md`, all in the same direction: it
claimed things are missing that are built.

- **§1, predicate writes** — the section headed "Missing: the two that matter
  most" — is built. `delete_where` and `update_where` in the kernel,
  `DeleteWhere`/`UpdateWhere` on the wire, all three clients, and inside a
  batch.
- **§2, relations do not cross the wire** — also built. A `Related` RPC,
  `related()` in Python and TypeScript, `related.go` in Go, plus `through` and
  nested loading.
- **The validations row** said `CHECK` "cannot name a column, report more than
  one failure, or reach a client". It does all three.

## Why

The security review's list of what it did not examine paid for itself three
times today, so I went looking for the product's equivalent — and
`orm-comparison.md` is it: a feature audit against seven ORMs, sorted into
*missing*, *refused* and *forbidden*, with the grep that established each claim
written next to it.

**It contradicts itself.** The section titled "Missing: the two that matter
most" describes predicate writes and cross-language relations. Six hundred
lines further down, the same file's plan marks both `### P1 — … — **built**`
and `### P2 — … — **built**`. Whoever built them updated the plan and not the
audit, and the audit is the part at the top.

The cost is not academic. A reader deciding whether to use this reads the
first gap section and is told, with evidence and a grep, that
`DELETE FROM sessions WHERE expires_at < now()` "is not expressible" and that
batched relationship loading is "reachable **only from Rust**". Both are false,
and the second names the three client audiences it claims were left out —
audiences that have had the feature since P2.

The file predicted this precisely:

> The grep that established each one is named so the next reader can re-run it
> rather than trust this file — which will go stale, and this paragraph is the
> instruction for what to do when it has.

So I re-ran them. `grep -rn "fn delete_many\|delete_where" crates/`, cited as
returning nothing, returns the kernel method and its tests. `grep -cin
"relat\|preload\|include"` over the proto, cited as returning 1, returns the
`Related` request, response and relation messages.

## Alternatives rejected

**Delete the two sections.** Shortest, and it throws away the most useful thing
in the file. What a project judged its two worst gaps, argued for at length,
and then closed is better evidence that the audit was honest than a document
that has never admitted to a gap. Struck through, with the built state above
each, keeps both.

**Rewrite them as prose about what exists.** The feature docs already do that.
This file's job is the comparison, and a comparison whose gap list is a
changelog of closed gaps has lost its shape.

**Renumber the remaining gaps so "the two that matter most" names two that are
still open.** Tempting and wrong twice: the numbering is cited from the plan
sections below, and the two that matter most *were* those two — that is a
historical claim about a judgement, not a live ranking.

**Fix the validations row by deleting it.** Hooks really are missing, and they
are the part of that row a user would notice. The row now says which three of
its four claims are stale and that the fourth — lifecycle hooks — is designed
out in `validation.md` rather than merely absent, which is a different category
in this file's own taxonomy.

## Evidence

Each correction was established by running the grep the original cited, and by
opening the files:

| claim | what is there now |
| --- | --- |
| no `delete_where` anywhere in `crates/` | `record.rs`, two `pub async fn`, plus `DeleteWhere`/`UpdateWhere` RPCs |
| Python's methods are "insert, update, delete, get, query, join, aggregate, explain, transaction, transact" | `related()` at `client.py:1114`, `delete_where` and `update_where` besides |
| relations reachable only from Rust | `clients/go/slate/related.go`, `clients/go/slate/predicate_write.go`, and the same in Python and TypeScript |
| `CHECK` cannot name a column | `CheckDef::column`, `constraint.rs:167` |
| `CHECK` cannot report more than one failure | `SchemaError::CheckViolation { violations: Vec<CheckFailure> }` — "**Every** failing check, not the first" |
| `CHECK` cannot reach a client | typed in `clients/go/slate/error.go`, with `test_details.py`, `details_test.go` and `details.test.ts` |

I checked the Go client twice, and the first answer was wrong: grepping
`clients/go/slate/client.go` for `DeleteWhere|Related` returns nothing, because
Go's are in `predicate_write.go` and `related.go`. Had I stopped there I would
have "confirmed" a gap that is not there and written it up. Recorded because it
is the same mistake as grepping for the word `redact` this afternoon — a grep
scoped to where I expected the answer is not a search.

`python3 site/check/docs.py`: every relative link resolves.

No code changed, so there is nothing to mutation-test.

## What this does not do

~~**It does not re-verify the rest of the gap table.** Nine rows remain under
"Missing", and I checked three of them — set operations (still refused by the
SQL front end, with a reasoned message, so the row is right), the array type
(`ValueType` has nine variants and none is `Array`, confirmed while working on
the codec today), and validations. The other six — generated migrations from a
schema diff, window functions, CTEs, views, full-text search, seed factories —
I did not re-run the greps for. Given that three of the ones I did check were
stale, the prior on the rest is not good.~~

> **Done, and the prior was wrong: all six are accurate.**
>
> | row | checked |
> | --- | --- |
> | window functions | `Aggregate` has exactly the seven the row lists — `Count, CountColumn, Min, Max, Sum, Avg, CountDistinct` — and no frame or partition |
> | CTEs / recursive | no `WITH`, no `RECURSIVE`, no CTE node anywhere in the SQL front end or the spec |
> | views | no `ViewDef`, no `CREATE VIEW`, nothing in `slate-schema` or the kernel |
> | full-text search | nothing; `LIKE`/`ILIKE`/regex, as the row says |
> | generated migrations from a diff | `migrate.rs` is `plan`, `apply`, `migrate`, `verify`, `fingerprint`, `stored_state` — it reads a catalog you wrote and plans the diff; nothing writes the target catalog |
> | seed factories | `--seed <file>` on `slate-serverd` and nothing else; no client method, no library entry point |
>
> So nine rows: three were stale and six were right. The staleness was
> concentrated in the rows describing things somebody then went and built,
> which is the obvious place for it in hindsight and was not the reason I
> checked them.

**It does not close any of the remaining gaps.** They are feature work, and
several are in tension with stated architecture: the SQL front end's refusal of
`UNION` is not an omission but a consequence of a statement compiling to one
`QuerySpec`. Whether that trade is still right is a design conversation this
entry does not have.

**Nothing stops this recurring.** The plan sections are marked `**built**` by
hand and the audit sections are not, and no check relates them. A rule of the
shape "a plan item marked built must have its audit section struck through"
would need to know which audit section a plan item belongs to, which is prose.
I considered and did not build it.
