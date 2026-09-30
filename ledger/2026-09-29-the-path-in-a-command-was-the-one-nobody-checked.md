# Widening a citation guard past `docs/` found a stale path, and then found a hole in the widening

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `scripts/check_cited_files.py`,
  `scripts/test_check_cited_files.py`, `site/README.md`
- **Kind:** fix

## What changed

`scripts/check_cited_files.py` read `docs/*.md` and matched a code span that
was *wholly* a path. It now reads `README.md`, `CLAUDE.md` and `site/` as well,
reads the `<code>` the site's HTML uses, reads each whitespace-separated word
*inside* a span rather than only the span entire, strips a trailing `:<line>`
or `::<item>` so a place within a file resolves as the file, and treats a suffix matching two files as a
finding unless those files hold identical bytes. An exact tracked path no
longer reads as an ambiguous suffix of a deeper one. 109 cited paths across 14
pages became 129 across 29. Its fixture suite went from 7 cases to 17.

One stale path fell out: `site/README.md` said the SQL front end is
`crates/slate-wasm/src/sql.rs`. It has been `crates/slate-sql/src/sql.rs` since
`6f2d3db` ("Extract slate-sql: the parser and the query spec"), and the site
was not updated.

## Why

Two caveats in
`ledger/2026-09-29-mutating-the-real-tree-found-a-class-nothing-checked.md`
asked for exactly this:

> **The suffix rule accepts a citation that resolves to the wrong file.**
> `tests/oracle.rs` would match any `tests/oracle.rs` in the tree, and there
> could be two. Today there is one of each of the twenty, checked by hand while
> writing this; nothing keeps it that way.

> **Only `docs/` is in scope.** `README.md`, `CLAUDE.md` and `site/` cite paths
> too, and `site/check/docs.py` checks the site's links but not its file
> citations.

Both were real, and the second was worth more than it looked: the guard's own
docstring recorded a null result — 108 of 109 resolving, nothing caught — and
the first page put in scope had a path a rename had left behind for as long as
`slate-sql` has existed. A reader of the site's README following it lands
nowhere. That is the failure this guard was written for, and it was sitting
outside the guard's reach the whole time.

The widening then produced two findings against itself, which is the part
worth reading.

**The site's HTML does not use backticks.** `site/docs/*.html` marks a path
with `<code>`, and a pattern that only read backticks would have walked ten
pages, found nothing, and reported `site/` as covered. This repository has met
that shape enough times to have a name for it — a check that never fires is a
check nobody has debugged — and this instance arrived *inside* the change that
was widening the scope.

**A path inside a command was invisible, and a mutation proved it.** Mutating
`` `python3 scripts/reclaim.py` `` in `CLAUDE.md` to name a script that does
not exist **survived** (record below). The pattern was anchored to the span's
delimiters, so a span holding a command matched nothing at all. That is the
citation most worth checking: a command naming a script that moved does not
run, and `CLAUDE.md` is almost entirely commands. Reading word by word inside
the span catches it — and the same read found two `file.rs:12` locations in
scope, `crates/slate-derive/src/lib.rs:629` and
`crates/slate-sql/tests/front_end.rs:185`, which had been invisible for the
same reason — and three more written `file.rs::Symbol`, which is how
`docs/security-review.md` names the item it is about. One of those,
`crates/slate-serverd/src/seed.rs`, is cited *only* that way: the security
review, the document whose whole subject is citations that went stale, named a
path nothing checked.

## Alternatives rejected

**Disambiguate the two `google/rpc/*.proto` citations by writing a full path.**
The obvious fix for the ambiguity the widening surfaced, and wrong on the
facts. `docs/orm-comparison.md` names those files as *protobuf descriptors* —
the argument is that two non-identical descriptors under one name are a hard
error in protobuf's default pool — and the name is exactly the thing shared by
both vendored copies and by upstream. Naming
`crates/slate-server/proto/google/rpc/status.proto` would make the sentence
say something it does not mean. Comparing the bytes says what actually matters:
a reader sent to either copy reads the same file. It also earns a second job
for free — two vendored copies that *drift* stop being interchangeable and the
citation fails, and nothing else in the repository looks for that.

**A second roster, beside `EXTERNAL`, for names in a shared namespace.** The
other way to let those two through. Rejected because a roster is a list a
person must maintain, `EXTERNAL`'s own docstring argues that a roster nobody is
forced to edit goes stale, and a byte comparison needs no maintenance and
fails on its own when the premise stops holding. Two entries did not justify a
mechanism that can rot.

**Leave the exact-path case ambiguous and make `CLAUDE.md` write
`./scripts/mutate.py`.** `scripts/mutate.py` is also the tail of
`clients/python/scripts/mutate.py`, a different script for that package's own
suite. Editing the document to satisfy the guard is the wrong direction: a path
written from the root *is* the root's, because that is the base every reader
uses, and the alternative makes every root-relative citation in the repository
hostage to any file added below it. The rule changed instead.

**Match a path anywhere inside a word, not as a whole word.** It would catch
`--out=crates/x/src/lib.rs`, which whole-word matching skips. Rejected after
looking at what it costs: the matched *word* is what gets recorded, so
`--out=…` would be reported as a file that is not there — a false positive on a
path that resolves perfectly well. `check_cited_tests.py` measured an 87% false
positive rate on a looser rule and rejected it, and a guard that cries wolf is
a guard that gets skipped. The reach given up is one flag argument.

**Read fenced code blocks too.** `CLAUDE.md`'s fenced blocks hold real paths
(`cp ledger/TEMPLATE.md …`). Left alone: a fence usually holds a worked example
with placeholder paths, and the span rule already reaches every inline command,
which is where the load-bearing citations are. Recorded below as a gap rather
than done badly.

## Evidence

- `python3 scripts/check_cited_files.py`: **109 paths across 14 `docs/` pages**
  before, **128 across 29 pages** after. Where the 19 come from, measured one
  widening at a time:

  | reading | distinct paths |
  | --- | --- |
  | `docs/*.md`, backticks, whole span | 109 |
  | + `README.md`, `CLAUDE.md`, `site/*.md` | 126 |
  | + `*.html` and `<code>` | 127 |
  | + words inside a span | 127 |
  | + `file:line` and `file::item` places | 129 |

  Reading inside a span adds **no distinct path** and 3 occurrences (188 → 191).
  It is in for the hole it closes, not the reach it buys, and the count says so.

- The stale path, found by the widening and not by argument: `site/README.md`
  cited `crates/slate-wasm/src/sql.rs`; `git log --diff-filter=D` puts its
  deletion in `6f2d3db`, and `crates/slate-sql/src/sql.rs` is where it went.
- `python3 scripts/test_check_cited_files.py`: **17 passed, 0 failed**, up from
  7 cases.
- `sh scripts/check.sh`: **85 passed, all of them**, exit 0.
- **Thirteen mutations against the fixture suite**, in four runs. Twelve were
  caught on the first attempt; the thirteenth survived and became a test:
  - `ledger/mutations/20260929T160849-scripts-check-cited-files-py.json` — six:
    the exact-path rule dropped; `differ` never differing and always differing;
    the `<code>` arm disabled; the scope narrowed back to `docs/`; HTML pages
    dropped.
  - `ledger/mutations/20260929T161115-scripts-check-cited-files-py.json` — the
    span read whole again, and the `./` exclusion removed. Both caught, and a
    third — matching loosely rather than whole — **survived**, which was a
    missing test rather than redundant code: nothing in the fixture held a word
    with a path inside it. `a path inside a longer word is not read as a
    citation` is that test.
  - `ledger/mutations/20260929T161216-scripts-check-cited-files-py.json` — the
    survivor above, now caught, and the location rule.
  - `ledger/mutations/20260929T162326-scripts-check-cited-files-py.json` — the
    two halves of `AT_PLACE` removed one at a time, so neither form rides on
    the other's test.
- **Four mutation runs against the real tree**, through
  `scripts/mutate_guard.py`, because a fixture proves the rules refuse and only
  the real tree proves they are pointed at anything. Two caught, one survivor,
  and that survivor re-run and caught after the fix:
  - `ledger/mutations/20260929T160917-readme-md.json` — `README.md` names an
    example that is not there. Caught.
  - `ledger/mutations/20260929T161108-site-docs-limits-html.json` — a site page
    names a design note that is not there. Caught.
  - `ledger/mutations/20260929T160923-claude-md.json` — `CLAUDE.md`'s
    `python3 scripts/reclaim.py` names a script that is not there.
    **SURVIVED.** This is the finding, not a footnote: it is what says the
    delimiter-anchored pattern could not see a command.
  - `ledger/mutations/20260929T161101-claude-md.json` — the same mutation after
    the span rule. Caught.

## What this does not do

**Fenced code blocks are still unread.** The span pattern does not cross a
newline, so a Markdown ```` ``` ```` block contributes nothing. Counted rather
than guessed at: the fenced blocks in scope hold **10 distinct paths, all of
them tracked**, and **5** that no inline citation also names —
`crates/slate-headbench/run.sh`, `crates/slate-slatedb/run.sh`,
`ledger/TEMPLATE.md`, `site/build-wasm.sh` and `site/data/bucket.json`. So
reading fences would cover five more paths and find nothing today. Left out
because a fence is where a placeholder lives (`path/to/your.rs`), and a rule
that reports a placeholder as a missing file is the false-positive trade this
guard already declined once. Five paths did not buy that risk; the number is
here so the next person can weigh it rather than re-measure.

**`ledger/` is still out of scope, and that is a principle, not an omission.**
An entry is a dated record. A path that has since moved is provenance, and
correcting it would be rewriting history to make a check pass. This is the same
reasoning `check_cited_tests.py` sets out at length, restated because the
widening makes the question live again.

**A path inside a longer word is skipped.** `--out=crates/x/src/lib.rs` is not
checked, by choice. Counted: **3** words in scope contain a path without being
one, and all three were `file.rs::Symbol` — which is now handled as a place
within a file rather than as a substring. So after this change the rule skips
**nothing** in the tree as it stands, and the cost of the trade is entirely
hypothetical. It will not stay that way; a `--out=` or a parenthesised path
arriving later is silently unchecked, and nothing announces that.

**The identical-copies rule compares bytes, not meaning.** Two files that
happen to be identical today and are conceptually unrelated would pass, and the
guard would start failing when one changed — a correct failure for the wrong
reason. The two real instances are vendored copies of one upstream file, where
identity is the whole point; nothing enforces that a future pair is the same
kind of thing.

**Nothing checks a line number.** A `file.rs:629` location resolves as
`file.rs`, and whether line 629 is still the line meant is not checkable and is
not checked. Line numbers go stale faster than paths; this guard makes no claim
about them.

**The site's link checker and this one still do not know about each other.**
`site/check/docs.py` checks the site's `href`s; this checks its file citations.
A path written as a link rather than as code is read by neither.
