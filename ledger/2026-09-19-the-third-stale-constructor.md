# A subclass forwarding its parent's keywords by hand went stale for the third time, so it stopped doing that

- **Date:** 2026-09-19
- **Author:** Claude Opus (session 015RgS5KMW89YEg1UGgZvfDo)
- **Touches:** `clients/python/src/slate/errors.py`,
  `clients/python/tests/test_errors.py`
- **Kind:** fix

## What changed

`NotLeader.__init__` is gone. It existed to call `super().__init__` with each
of the base's keywords spelled out and then set `self.leader` from the
trailers; `leader` is now a `@property` reading the same trailers on demand.
Plus a test that constructs every class in the `SlateError` hierarchy with
every keyword `from_rpc_error` passes.

## Why

CI run 187 went red on two tests with
`TypeError: NotLeader.__init__() got an unexpected keyword argument
'violations'`. Adding `violations` to the base in the previous change left the
one subclass that re-declared the base's signature behind.

The comment sitting above that constructor said, in as many words, that this
had already happened **twice**, and argued for keeping it explicit anyway
because `**kwargs` would forward a typo as readily as a field. The argument was
sound and the conclusion was wrong: it treated "explicit forwarding" and
"`**kwargs`" as the only two options, and never asked whether the constructor
needed to exist. It did not. Everything it did apart from the forwarding was
one attribute derived from a field the base already stores and nothing mutates.

Worth recording separately: the reason it reached CI is the same shape as the
lint fix one commit earlier. `pytest tests/test_details.py` was run — the file
that had changed — and not the suite. The change was to `errors.py`, which the
whole suite depends on.

## Alternatives rejected

**`**kwargs`.** One line, closes it forever, and the old comment's objection to
it stands: a caller misspelling `resaon=` would be forwarded to the base and
raise there, which is a worse error message than the one this produced. It also
makes the class's signature unreadable to a type checker and to anyone opening
the file.

**Add `violations` to the override and move on.** What the previous two
occurrences did. It is the smallest diff and it guarantees a fourth.

**Keep the constructor and add the guard test alone.** The test would catch the
next drift on the next run, which is a real improvement over catching it in CI.
It still leaves a constructor whose only job is to stay in step with another
one. A property has no state to keep in step.

**A `__init_subclass__` that refuses an override.** Considered and rejected as
machinery: it would forbid a subclass from ever having a constructor, and a
future one might legitimately need to validate an argument. The test says the
same thing without a rule.

## Evidence

The two CI failures reproduce locally and pass after the change. The whole
Python client suite is **291 passed** — run this time rather than the one file
that had changed.

Two mutations of the new guard:

| mutation | result |
| --- | --- |
| restore the stale `__init__` on `NotLeader` | caught — 3 tests, the guard among them |
| `_descendants` walks no subclasses | caught — the `len(every) > 10` assertion |

The second matters more than it looks: a recursive walk that silently found
only the root would make the guard pass for one class and report nothing, which
is the failure mode of every "check them all" test. `len(every) > 10` is the
cheap oracle for it.

`ruff check .` and `ty check` in `clients/python` are clean — both run from
that directory this time, which is the other half of the previous entry.

## What this does not do

**Nothing checks the other two clients for the same shape.** Go's `Error` is a
struct with no constructor, so a new field is a zero value rather than a
`TypeError`; TypeScript's `SlateError` has one constructor and one subclass
count of zero. Neither can drift the way this did, and neither was checked
beyond reading them, which is a weaker statement than the Python one.

**The guard constructs; it does not call.** It proves every class *accepts* the
keywords `from_rpc_error` passes. It does not prove `from_rpc_error` passes the
ones the classes expect — a keyword removed from every class and from the call
site together would pass. That is not a drift, it is a rename, and it is what a
type checker is for.

**The keyword list in the test is hand-written.** It mirrors the call in
`from_rpc_error` and nothing forces it to. Deriving it from
`inspect.signature(SlateError.__init__)` was the alternative and it makes the
test agree with the base by construction — which is precisely the agreement
that was never in doubt. The list is here to mirror the *call site*, and that
is not introspectable.
