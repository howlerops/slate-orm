# The generator refuses an array column once, instead of failing two ways

- **Date:** 2026-09-20
- **Author:** Claude, closing a gap the previous entry only named
- **Touches:** `scripts/{codegen.py,test_codegen.py}`, `docs/{arrays.md,orm-comparison.md}`
- **Kind:** fix

## What changed

`codegen.UNSUPPORTED` and `refuse_unsupported`, called once before anything is
emitted. An array column is now refused with one message naming the table, the
column and what a generator would need. Two tests, one of them the never-fires
negative. `scripts/test_codegen.py` also gains the house-style
`N passed, M failed` summary and catches `Exception` rather than
`AssertionError`.

## Why

The previous entry closed with: *"no generator has a case for it. A table with
an array column will generate something wrong or nothing; I did not check
which, which is itself a gap."* Checking it found two answers, which is worse
than either.

The three **declaration** emitters look a type up with `.get()` and raise a
readable `Unknown` naming the language. The three **row** emitters index the
same tables directly — `PYTHON_FIELDS[column["type"]]` — and raise `KeyError`.
One cause, two failures, one of them unreadable, and which one a caller got
depended on the order the emitters happened to run in.

Refused rather than generated, because an array needs more than six new table
rows. Its declaration has to carry the element type — the fingerprint hashes
it, so a declaration without one *compiles* and is refused by the server, which
is precisely the failure `Unknown`'s own doc comment says this tool exists to
prevent. And its decoded form is element-typed in all three languages
(`Sequence[str]`, `[]string`, `string[]`), which the tables cannot express
because they are keyed by the column's type alone.

## Alternatives rejected

**Add "array" to the six type tables and be done.** Two lines per language, and
it produces a client that is refused by every request it makes: the declaration
would carry `ValueType.ARRAY` with no element, and the fingerprint would not
match. A generator that emits something plausible and wrong is the one outcome
this tool is written to avoid.

**Build it properly now** — element-aware native types and per-element encoders
in three languages. The right end state, and it is a refactor of twelve lookup
sites plus three encoders, at the end of a long session, in generated code
whose mistakes surface as a client that compiles and is refused. Refusing is
one function and a message that describes the work; building it is its own
task, and the message is now its specification.

**Leave the `KeyError`.** It is loud, and loud is most of what matters. It is
also unreadable, arrives from a different place depending on call order, and
says nothing about why — and a reader who hits it has no way to tell a missing
feature from a bug in the generator.

**Add a fourth `mutate.py` dialect for `all pass`.** That was the immediate fix
for "the harness cannot read this file's output". Rejected: `N passed, M
failed` is the house style every other `test_*.py` here prints, this one file
deviated, and a dialect for one deviation makes the deviation permanent.

## Evidence

Four mutations, all caught:

```
ok  the refusal never fires                              -> test_an_array_column_is_refused_once_and_says_what_is_missing
ok  the refusal does not name the column                 -> the same
ok  the reason is dropped from the message               -> the same
ok  every type is refused, not only the unsupported ones -> that test AND test_an_ordinary_catalog_is_not_refused
```

**The fourth needed two fixes to the harness before it could be scored, and
both are findings.**

*`test_codegen.py` printed `all pass`* rather than the house `N passed, M
failed`, so `mutate.py`'s `python` dialect matched nothing and reported *"the
command reported no test results at all"* — failure mode 2 in its own
docstring, met while using the tool written for it, for the third time this
session. The summary line now matches the house style.

*`main()` caught only `AssertionError`.* With the over-refusal mutation in
place, `test_an_ordinary_catalog_is_not_refused` raised `Unknown`, which the
runner did not catch, so the whole file aborted with a traceback and no summary
— again reporting nothing rather than a failure. This is the only `test_*.py`
here that calls functions directly rather than running subprocesses, so it is
the only one with the hole; it now catches `Exception` and reports the type.
**An aborted runner reads exactly like a clean one**, which is the same shape
as "a skip is green".

`python3 scripts/test_codegen.py`: 24 passed, 0 failed. `ruff check`: clean.
`scripts/check.sh`: 29 of 29.

## What this does not do

**It does not generate anything for an array.** That is the point, and it is
still a missing feature rather than a resolved one. A user with an array column
gets no generated row type in any language, and has to write the declaration by
hand — including the element type, or the server refuses it.

**The refusal list has one entry and no guard.** `UNSUPPORTED` is a hand-written
dict, and nothing holds it to the set of types the rest of the system supports.
A tenth type added upstream that codegen also cannot emit would fall through to
the old two-failure behaviour. The right guard is the one used elsewhere here —
derive the type space and require every member to be either spelled or
excluded with a reason — and the six type tables would need it too. Not built:
it is the same work as building the array case, and doing half of it would
leave a check that passes for the wrong reason.

**Nothing checks that the message stays true.** It says the declaration must
carry the element type and the decoded form is element-typed. Both are true
today and both are prose; if the type tables ever grow a way to express either,
the message becomes a stale explanation of why something is refused that no
longer needs to be.
