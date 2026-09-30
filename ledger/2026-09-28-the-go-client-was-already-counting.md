# The Go client was already counting, and I said it wasn't

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** `clients/go/slate/round_trip_test.go`, `docs/caveat-status.json`,
  `ledger/2026-09-28-round-trips-counted-at-the-clients-channel.md`
- **Kind:** test

## What changed

`clients/go/slate/round_trip_test.go`: twenty inserts are twenty `/Insert`
calls, the same twenty in a batch are one `/Batch` and no `/Insert`, counted at
the Go client's own unary interceptor.

And a correction. The entry written an hour earlier ends with

> **Only Python is instrumented.** … Go's `grpc.WithUnaryInterceptor` and
> TypeScript's interceptor option would each take an afternoon.

That is wrong, and it was wrong when written. Go has had the instrument since
relations landed: `dialRecording` in `clients/go/slate/related_test.go`
installs a unary interceptor, and
`TestRelatedIsOneRequestHoweverManyParents` asserts one `/Related` call for
fifty parents — the same measurement, on a different path, months old. The
sentence is struck in that entry with a pointer here.

What Go actually lacked was a **batch** count, which is what this adds. The
mechanism was there; one use of it was there; the use the caveat asked about
was not.

## Why the mistake is worth an entry rather than a quiet fix

It is the mirror image of the day's other recurring error, and finding it took
the same fifteen seconds of looking that would have prevented it.

Earlier today, eight greps produced hits that looked like refutations and were
not — a word appearing was read as the claim being false. Here the opposite:
**absence was assumed without a look.** I had just built the Python instrument,
knew exactly what it consisted of, and wrote "only Python is instrumented" from
that knowledge rather than from `git grep interceptor clients/go`. One command.

Both errors have the same root, which is worth naming because the day has now
produced it twice in opposite directions: *the cost of checking was lower than
the cost of being wrong, and I estimated neither.* A grep is a second. Reading
the file the grep hits is a minute. Publishing a claim about another client's
test coverage, in a ledger entry, on the strength of having written a different
client's test, is free at the time and expensive later — and this repository's
entries are read as evidence.

The narrower lesson for the tracker: the caveat this was recorded against said
"none of the three clients has a benchmark". That was *half* true, which is the
most expensive kind of caveat — a reader who checks finds the Go interceptor,
concludes the claim is covered, and moves on without noticing that the path it
covers is not the path in question.

## Alternatives rejected

**Amend the earlier entry in place.** It is dated today and append-only like
every other; the rule that settled the four scale entries this morning
(`ledger/2026-09-28-the-four-entries-that-were-true-when-they-were-written.md`)
would not apply, because the sentence was false when written rather than
overtaken. So it is struck in place with `~~…~~` and a blockquote pointing
here, which is what the repository does with a claim that was simply wrong —
`ledger/2026-09-14-frontend-tests-and-configurable-ports.md` is the precedent.

**Write the TypeScript one too, and close the caveat outright.** The TypeScript
client has no interceptor in any test, so it is the real remaining gap, and
doing all three would make a cleaner entry. Not done here because the
correction is the point of this one and bundling a third client's test into it
would bury the thing a reader needs to see. The residual now says exactly what
is missing.

**Reuse `dialRecording` unchanged.** It is in `related_test.go`, which is where
it belongs — it was written for that file's problem. Calling it from a new file
in the same package works and is what this does; moving it to the harness would
be tidier and would touch a file three other tests depend on for no gain today.

## Evidence

- `GOTOOLCHAIN=local go test ./slate -run TestABatchIsOneRequest -count=1`:
  PASS. Twenty `/Insert` calls for twenty singles; one `/Batch` and zero
  further `/Insert` for the same twenty batched.
- `GOTOOLCHAIN=local go test ./... -count=1` in `clients/go`: **ok**, 8.9s —
  the whole suite, because the Python version of this broke five tests in
  another file by leaving rows behind. Go's harness starts a fresh node per
  test, so this one cannot, and running it was how I confirmed that rather than
  assumed it.
- `GOTOOLCHAIN=local go vet ./slate`: clean, after it caught a missing `proto`
  import.
- The claim being corrected: `git grep -n "interceptor" clients/go/slate` finds
  `dialRecording` and its two callers in `related_test.go`, dated to when
  relations landed.

## What this does not do

**It does not instrument TypeScript.** No test there installs an interceptor,
so of the three clients one now counts two paths, one counts one path, and one
counts nothing. That is the accurate state and is recorded as the open residual
rather than the tidy "one of three" the struck sentence implied.

**Neither Go test counts paging.** The Python file counts `Query` calls per
keyset page; Go counts `/Related` and `/Batch`. Whether paging costs one call
per page in Go is unmeasured, and the three clients share a proto rather than a
paging implementation, so it does not follow.

**It does not check that the three agree.** Each client is measured against its
own expectation. A conformance case comparing round-trip counts across the
three would catch a client that quietly looped where the others batched, and
the conformance runner compares answers rather than request counts — which is
the same distinction `dialRecording`'s own comment draws about why counting was
needed at all.
