# Two of three rosters registered, and a filtered test run that hid the third

- **Date:** 2026-09-21
- **Author:** Claude Code (agent session)
- **Touches:** `examples/explorer/backends/go/schema/rows_test.go`
- **Kind:** fix

## What changed

`Posts` is registered in the Go decoder suite's `decoders` and `roundTrip`
maps. It was already registered in the Node and Python equivalents.

## Why

CI went red on `a37249c` with

```
ScanPosts is generated and nothing runs it; add it to `decoders`
Posts has a generated Row() and nothing runs it
roundTrip has 5 entries, schema.go declares 6
```

The three array tests were written and the map was not touched. The guard is
the same one that fired in Node and Python while I was adding those two — it
caught me there, I registered them, and then I wrote the Go tests as standalone
functions and never went back to the map.

**The reason it reached CI is the interesting half: I ran `go test ./schema/
-run Array`.** That filter excludes `TestEveryGeneratedDecoderIsExercised` and
`TestEveryGeneratedEncoderIsExercised`, which are the only two tests that could
have failed. `CLAUDE.md` says "running one file of a suite is not running the
suite" about `pytest tests/test_one.py`; a `-run` filter is the same mistake
with a different spelling, and it is worse, because a filter is chosen to match
the thing just written and therefore excludes precisely the guards that watch
for what was forgotten.

## Alternatives rejected

**Weakening the count check** so a table covered by standalone tests does not
need a map entry. Rejected: the count is what makes the guard a roster rather
than a suggestion, and "covered elsewhere" is exactly the claim a drifting list
makes about itself. The Node map has a `Books` entry whose body calls the
standalone test for the same reason; the Go `Posts` entry now does too.

**A cross-language guard** that checks the three rosters agree. Rejected as
solving the wrong problem: all three guards worked. What failed was running
one of them.

## Evidence

`go test ./...` in `examples/explorer/backends/go`, unfiltered: ok.
`gofmt -l` empty, `go vet` clean. The other three demo suites re-run
unfiltered too — Node 32/32, the Python adapter 30, the web units 12/12 — since
the point of this entry is that a filtered run proves less than it looks.

Not demonstrated: that the Go guard fails without the entry. It did, in CI, on
`a37249c`, and the message is quoted above; I did not re-break it locally to
watch it again.

## What this does not do

**Nothing stops the next filtered run.** There is no mechanism here that makes
`-run` or `-k` or `pytest path/to/one.py` fail; the guard against it is
remembering, which is what just failed. The honest statement is that CI is the
backstop and it worked, one push later than it should have.

**It does not check the other language rosters are complete**, because each
already checks itself and all three were green. This entry exists because two
of the three were made green by the guard telling me, and the third was made
green by CI telling me, and those are the same event a step apart.
