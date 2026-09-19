# Where validation lives

`orm-comparison.md` names this as a design question rather than a missing
function, and says the honest answer is that nobody has worked it out:

> Those are a design question rather than a missing function — where does
> validation live when three clients in three languages share one catalog? —
> and the honest answer is that we have not worked it out. Probably the
> catalog, as declarative constraints beside `CHECK`, so the answer is the same
> in all three. That is a design note somebody should write before any of it is
> built.

This is that note. It concludes that the guess was right about *where* and
wrong about what the hard part is: the catalog is the only place a rule can
live and mean the same thing in three languages, but putting it there is
already done — `CHECK` is exactly that — and what is actually missing is that a
constraint cannot say which column it is about, cannot report more than one
failure, and is invisible to clients. Those gaps, plus one hole in the rule
language found by trying to use it, are the work — not a new rule language.

## What the question is

A validation is a rule about a row that the application cares about and the
storage engine could live without: an email that looks like an email, a title
between 1 and 80 characters, a status that is one of four words, a discount
that cannot exceed the price. Every ORM in the comparison has a place to put
them. This one has a catalog shared by a Rust library, a gRPC daemon and three
clients that do not share a runtime, so "in the model class" is not available.

## What the four actually give you

Worth separating, because they are usually named together and are three
different mechanisms.

**Ecto changesets** are the strongest design of the four, and the least like a
hook. A changeset is a *value*: `cast` takes the params, `validate_*` functions
add errors, and the result is data you can inspect before anything touches the
database. Validation is not attached to the struct and does not run implicitly.
Uniqueness is the interesting case — Ecto tells you plainly that
`unsafe_validate_unique` races, and that the real guarantee is
`unique_constraint` reading back a database index violation.

**ActiveRecord validations** declare rules on the model and run them on `save`,
accumulating `errors` per attribute. The per-attribute error collection is the
part users actually want: a form needs every problem at once, keyed by field.
`validates_uniqueness_of` carries the same documented race as Ecto's.

**SQLAlchemy** has `@validates` and a broad event system — the most powerful
and the least declarative. It is a hook mechanism, and `orm-comparison.md`
already refuses its neighbour, the unit of work, for the same reason: implicit
ordering is the hardest thing in that library to reason about.

**Prisma** is the outlier and the most instructive. It has essentially no
application-level validation: the schema carries database constraints, and
everything else is pushed into the generated types and userland Zod. The most
recently designed ORM on the list looked at this problem and declined it,
leaning on codegen instead.

**Lifecycle hooks** — `before_save`, `after_create` — are a separate feature
that travels with validations and should not be confused with them. A
validation is a pure question about a row. A hook is arbitrary code with
effects, and it is where the action-at-a-distance complaints about
ActiveRecord come from.

## What is already here

More than the gap table implies. A row written through this system is already
refused for:

- **Type and nullability**, from the column definition.
- **The primary key**, and any unique index.
- **A foreign key**, checked against the parent table.
- **A `CHECK`**, which is a predicate string in the catalog — `predicate =
  "size >= 0"` — parsed into the same `Expr` the planner uses.

That last one is the important one, because it is already the answer the gap
table guessed at. These three are real, and were run through
`slate-serverd --check` rather than written from reading the parser:

```toml
[[tables.checks]]
name = "status_is_known"
predicate = "status in ('draft', 'review', 'published', 'archived')"

[[tables.checks]]
name = "title_is_not_empty"
predicate = "title like '_%'"

[[tables.checks]]
name = "discount_within_price"
predicate = "discount <= price"          # two columns, one row
```

(String literals inside a predicate are single-quoted, because the whole
predicate is already inside a TOML string. The parser says so when you get it
wrong, which is how the version of this note that had it backwards was caught.)

### The rule language is not the gap — a correction

`Expr` — the kernel's predicate type — holds `Compare`, `CompareColumns`,
`IsNull`, `Like`, `Matches`, `In`, `And`, `Or` and `Not`. `Matches` is a real
regular expression, added when the last ClickBench query needed one.

**It can be written in a check, and this note's first version said it could
not.** That claim was wrong and is withdrawn. The daemon's predicate language
spells a regular expression the way Postgres does — `~`, `~*`, `!~`, `!~*` —
not with a `matches` keyword, and the first draft tested `title matches '…'`,
got `expected a comparison after the column, found \`matches\``, and concluded
the feature was missing. The error was about a word that was never the syntax.

The grammar comment at the top of `lang/pred.rs` says so in as many words —
*"Every `Expr` variant is reachable, which is the property that makes this a
surface syntax for the kernel's predicates rather than a subset of them"* — and
the file has carried a parser test for `kind ~ '^a'` since regex was added.
Reading one screen further up would have caught it; running one more spelling
would have caught it.

So format validation is available today:

```toml
[[tables.checks]]
name = "title_length"
predicate = "title ~ '^.{1,80}$'"
```

`schema.rs` has two tests for this — one that the check accepts a short title
and rejects both an over-long one and an empty one, which is the bound `LIKE`
provably cannot express, and one that a pattern which does not compile is
refused at startup rather than silently matching nothing.

**What this changes about the recommendation below:** the prerequisite is gone.
There were never four things, only three, and none of them is in the rule
language. It also removes the one item that would have been a code change to
the parser, which makes the remaining three purely about *reporting* and
*publishing* constraints — a narrower and more coherent piece of work than the
note originally described.

So: no hole in the rule language, and three other things.

### 1. A check cannot say which column it is about

The refusal is `row violates check `title_length` on table `docs``. It names
the check and the table. It does not name the column, the offending value, or
anything a form could put next to a field. A caller gets a string and has to
parse a name it chose, which is the kind of contract that works until somebody
renames a check.

### 2. One failure, not all of them

The write path stops at the first check that fails. A form with four bad fields
needs four round trips to discover them, and each one shows the user a single
error — the behaviour that made per-attribute `errors` the headline feature of
ActiveRecord validations.

### 3. Clients cannot see the constraints

`--print-schema` publishes columns, indexes, the primary key and the tenant
column. It does **not** publish checks or foreign keys, and the schema
fingerprint deliberately excludes them (the migration refusal says so: "Renames,
CHECKs and foreign keys are not covered"). So no client can evaluate a rule
before sending, and no generated type can reflect one. Every validation costs a
round trip, and the rule exists in exactly one place that no client can read.

## What the architecture forbids

Three things are not available and should be said before anyone designs around
them.

**A validation cannot depend on the caller.** `constant_predicate` refuses
`:principal` in a check, deliberately: a check is evaluated with no caller, and
the assertion that it is caller-independent costs a tree walk at startup.
Caller-dependent rules are row-level security, which is a different mechanism
with a different audit story. "Only an editor may set `status = published`"
belongs in a policy, not a validation.

**A validation cannot query.** `Expr` evaluates against one row. Uniqueness,
existence in another table, and "no more than ten per account" are not
expressible and should not be made so — a predicate that issues reads is a
predicate whose cost is unbounded on the write path. Uniqueness has an index;
existence has a foreign key; the third has no answer here, which is worth
saying rather than hiding.

**Arbitrary code cannot run.** There is no place to put a `before_save` that
would be the same in Rust, Python, Go and TypeScript. This is not a limitation
to work around; it is the reason the catalog is the answer.

## The recommendation

**Keep the rule language. Fix the three gaps. Refuse hooks.**

1. ~~**Give a check a column and a message.**~~ **Built.** Two optional fields
   beside `name` and `predicate`: `column`, naming the field the error belongs
   to, and `message`, the text a form shows. Optional because a cross-column
   check like `discount <= price` has no single column, and forcing one would
   make the answer a lie. A `column` naming no column on the table is refused
   at startup rather than at the write.

   The error carries both. `message` rides in the status text, because it is a
   sentence; `column` rides in `ErrorInfo.metadata`, because a caller parsing a
   field name out of prose is the contract this exists to avoid. A check
   violation also got its own reason token, `CHECK_VIOLATION`, split out of the
   generic `SCHEMA`: the caller's *data* being wrong is retryable after editing
   a field and the caller's *schema* being wrong is not.

2. ~~**Collect every failure.**~~ **Built**, and the hedge in this paragraph
   turned out to be unnecessary. It proposed making the collecting behaviour
   opt-in so the common case could still stop early; there is no common case to
   protect. **A row that passes already evaluates every check**, because that
   is what passing means. Short-circuiting only ever saved work on the
   *failure* path — the rare one — and the saving is a few predicate
   evaluations against one in-memory row. So: no flag, no second code path, and
   `violations` is always the whole set, in declaration order.

   `SchemaError::CheckViolation` now holds `Vec<CheckFailure>`. One failure
   renders exactly as it did before several were possible; several render as a
   list. On the wire each failure gets `check.N`, `column.N` and `message.N`
   beside a `violations` count, and the unindexed `check`/`column` stay as the
   first failure for a client that shows one error at a time.

3. ~~**Publish the constraints.**~~ **Built, in part.** `--print-schema` now
   carries `checks` (name, column, message, and the text the predicate was
   parsed from) and `foreign_keys`, and `scripts/codegen.py` generates from
   both. `status in ('draft', 'live')` over a `str` column becomes
   `Literal["draft", "live"]` in Python and `"draft" | "live"` in TypeScript;
   Go has no union of string literals, so it gets the values as a slice and the
   field stays `string`.

   **The half that is not built is client-side evaluation**, which is what
   "refuse a bad row without a round trip" meant. That needs an expression
   evaluator in three languages — a second, third and fourth implementation of
   `lang/pred.rs` — and this note's own open question about regular expressions
   across three engines is the first of many ways they would disagree. What
   exists is the rule as *data*: enough to show it beside a field, generate a
   type from the simple shape, and map a refusal back to a column. The server
   remains the only thing that evaluates anything.

Do them in that order. (1) and (2) are server-side and independently useful.
(3) is the largest and depends on the generated-types work, and it introduces
the one genuinely new risk: a rule evaluated in two places can disagree. The
server stays authoritative — a client-side check is an optimisation and a
better error, never the enforcement — and the schema fingerprint is what stops
a client evaluating a stale rule, once checks are in it.

**Lifecycle hooks are refused**, and belong in the "Refused, with the
reasoning" section rather than the gap table. There is no place to run them
that all three clients share, a hook that ran only on the daemon would be
invisible to the Rust library, and one that ran only in a client would be
absent from the other two. The cases they are used for divide cleanly:
timestamps are managed columns, which now exist; derived values are computed
columns; audit trails are the request log; and cascading writes are `transact`,
explicitly. What is left is the action-at-a-distance that ActiveRecord is
criticised for.

## What this note does not settle

**Whether `message` should be translatable.** A single string in the catalog is
one language. Every real form eventually wants a key rather than a sentence,
and a key means the catalog carries an identifier whose meaning lives somewhere
else entirely. Both options are defensible and neither was worked through here.

**How a client-side rule would evaluate a regular expression.** Not a problem
today, because a check cannot hold one — but it becomes the sharpest edge in
recommendation (3) the moment the missing keyword is added. Handing a pattern
to Python, Go and TypeScript means three regex engines with three subtly
different dialects evaluating something the server compiled with a fourth, and
a client that disagrees with the server about whether a row is valid is worse
than a client that does not try. The conservative answer is to publish only the
predicates whose client-side evaluation is provably identical — `In`,
`Compare`, `IsNull` — and keep patterns server-side. That ordering is an
argument for doing the parser keyword and the publishing work in that order,
and not together.

**Whether any of this is wanted.** No user has asked. The gap table lists
validations because seven other ORMs have them, which is a reason to have an
answer and not a reason to build one. The three gaps above are worth fixing on
their own merits — an error that names its column is better whatever the
feature is called.
