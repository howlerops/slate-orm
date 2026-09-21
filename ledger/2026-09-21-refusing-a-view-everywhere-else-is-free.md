# Refusing a view everywhere else turns out to be free, and that sets the build order

- **Date:** 2026-09-21
- **Author:** Claude Opus 5, closing F5a
- **Touches:** `docs/views.md`
- **Kind:** docs

## What changed

A new §3a in `views.md`: tracing what decision 1 — "a view must not be a
`TableDef`" — already buys, and what follows for the order the feature gets
built in.

## Why

I was about to start building, and stopped to check one thing first:

```rust
pub fn table_by_name(&self, name: &str) -> Option<&TableDef> {
    self.tables.iter().find(|t| t.name() == name)
}
```

Every path that turns a request's table name into a `TableDef` goes through
that — `service.rs`'s `table`, and the two converters in `convert.rs` — and
`scripts/check_handlers.py` rule 1 already holds every handler to one of them.
So a view kept outside the `Catalog` is refused by every path that exists,
**without a line being written**. Not by a check somebody added: by there
being nothing to find.

That is decision 4 — writes through a view are refused — delivered by
construction rather than by vigilance. It also refuses joins, chains,
aggregates and paging, which is the right default for all of them today.

Which decides the order: **declare first, read last.** Step one is
`[[views]]` in the TOML with a registry beside the catalog, at which point a
view can be declared and every use is refused — a complete, safe state rather
than a half-feature. Step two opts one read path in deliberately.

Building it the other way round — teach the reader about views, then remember
to refuse the writers — is the shape of every finding in
`security-review.md`: a fix that covered one path of several, three times in
one session by that document's own count. This order has no intermediate
state in which it could happen.

## Alternatives rejected

**Build it now rather than write this down.** The finding is the part most at
risk of being lost: it lives in a three-line function and a guard script, and
it is not visible from anywhere the next person would look. The 500 lines it
enables are ordinary work; the reason they are safe in that order is not.

**Add an explicit "is this a view?" refusal to each write path anyway, for
clarity.** It reads as defence in depth and is the opposite: a reader who
finds five explicit refusals concludes the refusal is *maintained by those
five*, and the sixth path gets added without one. The structural property is
stronger precisely because there is nothing to keep in step.

**Put views in the `Catalog` with a flag, and check the flag.** This is
decision 1 again, arrived at from the other side. A flag is a thing to forget;
`row_filter_with` keys on `TableId` and would read a view's own id, which §1
shows returns `Expr::True` over the base rows. The measurement here is why
that decision pays off twice.

## Evidence

The three call sites, from
`grep -rn "table_by_name" crates/slate-server/src crates/slate-serverd/src`:
`convert.rs:2056` (a query's table), `convert.rs:2659` (a join input),
`service.rs:426` (`Head::table`). Every other hit is a test. Rule 1 of
`check_handlers.py` is quoted in the note.

No code changed, so there is nothing to measure. `python3 site/check/docs.py`
green.

## What this does not do

- **It builds no view.** F5a is closed — the crate, the lowering, the tests —
  and F5b is the feature, with the order above as its first line.
- **It does not check the property it relies on.** Nothing fails if a future
  `Catalog` gains a second lookup that a view could satisfy, or if a handler
  resolves a name some other way. `check_handlers.py` rule 1 covers the
  handlers and not the catalog. A guard is writable — assert `table_by_name`
  is the only public name-to-`TableDef` lookup — and was not written, because
  today there is no view to leak and writing it before the feature would be
  guarding an empty room.
- **It leaves the refusal's wording wrong**, once views exist: "no table named
  `recent_books`" about a view that does exist. Named in the note and in F5b.
