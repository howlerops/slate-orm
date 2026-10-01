# A check the derive could not declare, and a caveat that understated it

- **Date:** 2026-10-01
- **Author:** Claude Code (session: work down the open caveat backlog)
- **Touches:** `crates/slate-derive`, `crates/slate-orm`, `docs/validation.md`,
  `docs/orm-comparison.md`
- **Kind:** feature

## What changed

`#[derive(Record)]` takes a `check(...)` attribute, on the struct or on a
field:

```rust
#[record(check(
    name = "priced",
    predicate(Expr::compare(price, CmpOp::Ge, Value::I64(0))),
    column = "price",
    message = "Price cannot be negative."
))]
struct Item {
    #[record(pk)] id: u64,
    price: i64,
    #[record(rename = "label")]
    #[record(check(name = "named", predicate(Expr::matches(title, "^.{1,8}$"))))]
    title: String,
}
```

`predicate(...)` is the same kind of expression `only_where` takes, for the
same two reasons, and every field of the struct names its own `Ordinal` inside
it. A check written on a field defaults `column` to that field, resolved
**after** any `rename`. A `column` naming nothing is a compile error, and so
are two checks with one name.

## Why

Two open caveats said the derive could not declare a check's `column` or
`message`:

> `#[derive(Record)]` cannot declare either field. A check written in Rust
> goes through `CheckDef` directly and can call the builders; one written
> through the derive macro's attribute cannot yet say `column = "..."`.
> — `ledger/2026-09-19-a-check-that-names-its-field.md`

**Both understated it, and the phrase "the derive macro's attribute" is where
they went wrong: there was no such attribute.** `grep -n 'is_ident("' crates/slate-derive/src/lib.rs`
listed eleven options on the struct and eight on a field, and `check` was
neither. The derive could declare no check at all, so the surface most likely
to be read as *how you declare a table in Rust* could not express a feature
the TOML loader has had since 2026-09-19. A Rust caller wanting one had to
abandon the derive and hand-write a `TableDef` — which is exactly the
workaround `examples/helpdesk` is built around for text indexes, and
`ledger/2026-09-30-an-application-written-against-the-rust-surface.md`
recorded how much that costs: `Record::table()` is what every write hands the
store, so a hand-written table means a newtype and a test holding the two
shapes together.

A caveat that names a smaller gap than the one that exists is worse than one
that names none, because it reads as scoped. This one would have been read as
"add two optional fields to an existing attribute" — half an hour — by anyone
planning from the tracker.

## The compile error the rest of the system cannot make

`column` is checked against the table in exactly one place:
`slate-serverd/src/schema.rs`, the TOML loader, at startup. `TableBuilder`
holds the string and never resolves it — it has no reason to, and
`TableDef::build` refuses a duplicate check name and an empty message and
nothing else about `column`.

So a derived check with a mistyped `column` would have reached a form as a
field name nothing renders beside: silently, at the first refused write, in
production. That is precisely the failure
`ledger/2026-09-19-a-check-that-names-its-field.md` records the field as
existing to prevent, arriving by the route the new attribute opened.
`validate_checks` refuses it at expansion with a span on the attribute, which
is strictly better than the startup refusal the TOML path gets.

## Alternatives rejected

**A string predicate, as the TOML loader takes.** `predicate = "price >= 0"`
reads better than `predicate(Expr::compare(price, CmpOp::Ge, Value::I64(0)))`
and is what a person coming from the TOML would expect. Rejected because the
parser is `slate-sql`, and the macro would have to either depend on it at
expansion (a proc macro parsing SQL, with the table shape it needs not yet
built) or emit a runtime `pred::parse(...)` call — which makes `slate-orm`
depend on `slate-sql` for every user of the derive, and turns a typo into a
panic at `Record::table()` rather than a compile error. The expression form is
what `only_where` already established, and the argument there holds unchanged:
a name resolved at runtime cannot be checked by the compiler, and building the
table is what evaluates the predicate.

**Leaving `column` unchecked, as `TableBuilder` does.** One fewer function and
no span to get right. Rejected on the paragraph above: nothing else in the
system would catch it, and the symptom is invisible until a write is refused.
The check costs one `fields.iter().any` over a list that is never long.

**Inferring `column` from the predicate.** `size >= 0` mentions exactly one
column and the macro can see it, so this was cheaper here than in the TOML
path. Rejected for the reason that path rejected it, which did not weaken:
it is right until the predicate mentions two, and then it is either wrong or
absent, and the author cannot tell which without reading the inference rule.
The field-level default is the version of this that is not a guess — it reads
the attribute's *position*, which the author chose.

**Refusing an explicit `column` on a field-level check.** It would make the
default unambiguous. Rejected because it makes the field position strictly
less expressive than the struct one: `discount <= price` wants to sit beside
`discount` and the author may want it beside `price`, and there is no reason
to make them move the attribute to say so.

**Not resolving `\0self` after `rename`.** The placeholder could have been the
field ident, which is simpler. Rejected and then *demonstrated* wrong: see the
mutation below. The ident is not a name any client ever sees.

## Evidence

**Five tests**, in `crates/slate-orm/tests/derive_checks.rs`, and three new
compile-fail doctests on the derive in `crates/slate-orm/src/lib.rs`.

**Six mutations of the macro, five caught**
(`ledger/mutations/20261001T003943-crates-slate-derive-src-lib-rs.json`):

| mutation | caught by |
|---|---|
| the column is never emitted | 4 tests |
| the message is never emitted | 3 tests |
| a field-level check gets no default column | 3 tests |
| no checks reach the builder at all | 4 tests |
| struct-level checks emitted after field-level ones | 2 tests |
| a field-level default resolves to the **field ident**, not the renamed column | the compiler — `validate_checks` refuses it |

The sixth scored `NOTHING RAN`, and the reason is the finding: mutating the
resolution to `ident.to_string()` makes the test struct's own check name
`title`, a column that does not exist after the `rename`, so `validate_checks`
refuses at expansion with its own message quoted verbatim in the run's output.
That is the guard working, and **`mutate.py` cannot score it** — a build
failure is its second documented lie, and it cannot tell a guard firing from
ENOSPC. Recorded below as what this does not do.

**Three mutations of the compile-fail doctests, all caught**
(`…T004012…` and `…T004025…`), which is the half that is easy to skip: a
`compile_fail` block passes when it fails to compile *for any reason*, so each
was mutated to remove only the mistake it is about and each then failed.

| mutation | the block then |
|---|---|
| `column = "emial"` → `"email"` | compiles, so the doctest fails |
| the duplicate check name differs | compiles, so the doctest fails |
| the non-`Expr` predicate becomes an `Expr` | compiles, so the doctest fails |

The first of those is the only demonstration anywhere that
`validate_checks`'s column rule fires, since the direct mutation cannot be
scored.

**A comment this run falsified.** The first draft's test prose said
"`CheckDef`'s `PartialEq` does compare the predicate, but only as a value",
and the equality test was written as though `assert_eq!(*Item::table(),
hand_written())` covered columns and messages. The mutation "the column is
never emitted" came back **not** naming that test, which sent me to
`constraint.rs`: `impl PartialEq for CheckDef` compares `self.name ==
other.name` and nothing else, and says so in its own doc comment. The prose
was wrong and the test was weaker than it read. Both are fixed — the equality
test now compares `(name, column, message)` field by field, and the module
doc says equality is a roster.

**And one number this commit moved by existing.**
`scripts/test_read_deliberate.py` went red on it: the later-entry signal's
second labelled row grew from 8 candidates to 9, and the ninth is *this
entry*, which shares "default", "column" and "assertion" with a claim about a
workbench playground's default-column assertion and has nothing to do with
it. The rank did not move — the answer was not a candidate before and is not
one now — so the only change is one more wrong answer on the shortlist. That
is the signal's failure mode at its cheapest, and `RANKS` records it rather
than widening the cut, for the reason that file already gives: tuning against
a two-row set chosen after the fact measures nothing.

**Not measured.** Nothing here is about speed. A check is evaluated per write
and the macro adds no evaluation that `TableDef::builder` did not already do;
whether three checks cost more than one on the write path was not timed and is
the schema layer's question, not this one's.

## What this does not do

**`mutate.py` cannot test a guard whose job is to fail the build.**
`validate_checks`'s column rule is tested here only through a compile-fail
doctest, at one remove: the doctest proves the rule *fires*, and the mutation
proves the doctest is about the rule rather than about a syntax error. The
direct mutation — break the resolution, watch the guard refuse — scores
`NOTHING RAN`, which is indistinguishable from ENOSPC. That is a real gap in
the tool for the whole class of compile-time guards, and this is the first one
written here; a `compile_fail` dialect that reads "this did *not* fail to
compile" as the failure is the shape of a fix and is not attempted.

**No client surface.** This is the Rust library only. A check declared this
way is published by `--print-schema` and reaches Python, Go and TypeScript
exactly as a TOML-declared one does, because both end in the same `CheckDef` —
but nothing in those clients is tested against a *derived* check, and the
conformance runner has no case for one. The daemon declares its tables in
TOML, so no deployed path reaches this attribute at all.

**It does not declare a foreign key.** `TableBuilder::foreign_key` is the
other constraint the derive cannot express, and it has the same shape of gap
and no caveat recording it. Not done here because `ForeignKeyBuilder` names a
parent `TableId` and the column-reference design is the `has_many`/`belongs_to`
question again; it deserves its own entry rather than a paragraph in this one.

**An empty `message` is still refused at `Record::table()`, not at compile
time.** `TableBuilder::build` catches it as `EmptyCheckMessage`, which this
macro's emission turns into a panic. It could be a compile error the way the
duplicate name now is — the macro has the literal in hand — and it is not,
because the panic names the check and arrives on the first `table()` call,
which is every test. The column rule is different only because nothing else
catches it at all.

**One struct, one shape of predicate.** The tests use `compare` and `matches`
over `I64` and `Str`. Nothing exercises a check over a decimal's scale, an
array, or a `CompareColumns` spanning two columns — that last being the case
`column` exists for, and the one the attribute's design argument rests on.
