# Guards against the three mistakes that recurred

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `.githooks/pre-commit`, `clients/python/{MANIFEST.in,tests/test_packaging.py}`,
  `clients/typescript/src/paths.ts` (new) and three files that now use it,
  `.githooks/test-pre-commit.sh` (new)
- **Kind:** repair

## What changed

Three things that went wrong more than once now have something that stops them.

1. **A file too large to push.** The hook refuses a staged file over 50 MB.
2. **Python packaging.** Four tests build the distribution and look inside it.
3. **Counting `..` segments.** One `findUpContaining` helper, used by the three
   TypeScript modules that each wrote their own.

The hook also gets the first test it has ever had — twelve cases, `sh` and
`git` only — for the reason in the next section.

## Why

Each of these cost real time in the last two days, and none of them was a hard
problem — they were all the kind of thing that is obvious once it fails and
invisible until then.

The size guard exists because `git add -A` swept a nested workspace's `target/`
into a commit: 2,412 files including a 102 MB binary, rejected at *push*, after
the commit existed, so the fix was rewriting history rather than amending.

The packaging tests exist because the TypeScript package shipped unimportable —
`exports` pointed at a path the build never produced — and all 37 of its tests
passed, because they import from `../src/` and never touch the entry point.
Python has the same shape of hole.

The path helper exists because the same `..`-counting bug was written three
times in one day.

## What they found

**The packaging tests failed on first run**, and the failure was real: the sdist
carried no `scripts/generate_proto.py`, because `scripts/` is not a package and
there was no `MANIFEST.in`. An sdist that cannot rebuild its own generated code
installs, imports, and is unmaintainable — which nothing else notices. Fixed.

## The mistake I nearly repeated

The packaging tests **passed under three mutations** when first written:
narrowing `packages.find` to one package, removing `py.typed` from
`package-data`, and deleting the `MANIFEST.in` line — all invisible.

The cause is exactly the class of thing these tests exist to catch. A
development checkout carries `src/slate_client.egg-info`, and setuptools reuses
its recorded file list rather than re-reading `pyproject.toml`. **A test of the
packaging was reading cached packaging output.** `_build` now copies the tree,
excluding `*.egg-info`, and builds from the copy.

After that, two of the three mutations fail. The third —  removing `py.typed`
from `package-data` — is genuinely equivalent: modern setuptools ships files
under the package directory anyway, so that line is belt-and-braces. Excluding
it through `MANIFEST.in` *does* fail the test, so the property is guarded even
though that one config line is not load-bearing. Recorded in the test rather
than left as a survivor with no explanation.

## The mistake I did repeat

The size guard, on its first real use, **rejected every commit in the
repository with no message at all.** Under `set -eu`:

```sh
[ "$size" -gt "$limit" ] && printf ...
```

When the last staged file is under the limit the test is false, so the `&&`
list is false, so the `while` is false, so the pipeline is false, so the
command substitution is false, so the *assignment* `too_big=$(...)` is false —
and `set -e` kills the script there, with status 1. Which is precisely what a
rejection looks like. I spent a while reading the ledger check for the bug.

An `if` instead of the `&&` fixes it. The interesting part is that I had
"tested the guard both ways" and still shipped this: the firing case ran, the
silent case ran, and both were run with the guard as the *last* thing in the
script rather than through a real commit, so the fatal status never mattered.

Hence `.githooks/test-pre-commit.sh`. It runs the hook the way git does — from
a work tree with an index already written — over the ledger cases, the
exemptions (merge, revert), and both size directions. It also asserts that
**every refusal prints something**, because the failure mode here was a silent
one and a silent guard is worse than none: it looks like the check working.

## Alternatives rejected

**A 100 MB limit, matching GitHub's.** 50 MB, because a source file that large
is a mistake well before it is a push failure, and the message says how to
proceed if it genuinely is not.

**A lint rule for `..` counting.** Would need a TypeScript AST pass to
distinguish `path.join(dir, "..", "x")` from a legitimate relative import, for
three call sites. One shared helper removes the need to detect the mistake by
making the right thing shorter than the wrong one.

**Putting the `.proto` in the Python sdist.** It lives in the server crate and
this package holds no copy; a copy could drift from the server's. The sdist
carries the generator, and the generator reads the schema from the repository.

## Evidence

The size guard is tested both ways — it fires above the limit and is silent
below — using an overridable threshold, so the test trips it with a kilobyte
instead of committing 60 MB of random bytes to find out.

Four Python packaging tests pass; three mutations kill three of them (the
fourth is equivalent, above). 49 TypeScript tests pass, and the built package
still resolves its bundled `.proto` from `dist/` — which is the exact failure
the helper is for.

`sh .githooks/test-pre-commit.sh`: 12 passed, 0 failed. Mutated three ways:

| mutation | result |
| --- | --- |
| restore the `&&` form of the size test | 14 failures, 10 of them "refused with no message" |
| make the size comparison never true | `a file over the limit` fails |
| drop the `[ -f "$path" ]` guard | **survives** |

The third is an equivalent mutant and stays as documentation of intent:
`size=$(wc -c < "$path" 2>/dev/null || echo 0)` already yields `0` for a staged
deletion, and `0` is not greater than any limit, so the `-f` test saves the
work and not the correctness.

## One the guards caught while being written

Renaming the TypeScript test output to `dist-test/` — done yesterday so `dist/`
holds exactly what ships — left it uncovered by `.gitignore`, and 28 compiled
files were staged into this very commit. Caught by reading `git status` rather
than by any guard here: the files are small, so the size limit says nothing.

`.gitignore` covers both directories now. It is the same root cause as the
`target/` incident — a build directory the ignore rules do not reach — and the
size guard only catches the loud version of it.

## What this does not do

The hook is a `pre-commit`, so it protects this repository's commits and not a
CI job or another clone that has not run `git config core.hooksPath`. Its test
suite is likewise run by hand — nothing runs `.githooks/test-pre-commit.sh` on
a schedule, so the next silent-refusal bug is caught only if somebody thinks to
run it.

The Go and Python clients have no equivalent of the TypeScript entry-point
test. Go's module path *is* its import path, so the failure mode does not exist;
Python's is now covered by the packaging tests.

Nothing stops a *fourth* nested workspace from being created outside the root
one — the demo's three backends already are. The size guard catches the
symptom, not the cause.
