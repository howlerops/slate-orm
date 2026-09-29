# A guard whose roster was its own docstring, and one that excused a live claim

- **Date:** 2026-09-29
- **Author:** an agent session, continuing from
  `ledger/2026-09-29-mutating-the-real-tree-found-a-class-nothing-checked.md`
- **Touches:** `scripts/check_toolchain_pins.py`,
  `scripts/test_check_toolchain_pins.py`, `docs/performance.md`,
  `docs/caveat-status.json`
- **Kind:** fix

## What changed

Two more guards were mutated against the real tree. Both were wrong, and
`check_toolchain_pins.py` was wrong twice over:

1. **It had never seen the only file in this repository that installs pinned Go
   tools.** `GO_INSTALL` required the `@version` in the same match as
   `go install`, and `clients/go/scripts/generate_proto.py` runs
   `subprocess.run(["go", "install", package])` with the pins in two constants
   fifty lines above. So the guard's entire roster was its own docstring's
   example and its own test's fixture, and it reported `2 pinned go install
   target(s) in 2 file(s)` about itself. `GO_INSTALL` and `PINNED` are now two
   patterns — an invocation anywhere, a pin anywhere — and the guard skips
   itself and its test by name, which is what gives the never-fires rule
   something to bite on.

2. **`DECIDES = re.compile(r"GOTOOLCHAIN")` matched a mention.**
   `generate_proto.py` explains its `GOTOOLCHAIN=auto` in twenty lines of
   comment above the assignment, so deleting the assignment left eight
   mentions and the guard green. It now wants an assignment, with comments
   stripped first.

And `check_cost_prose.py` was correct while a `<!-- not a cost-model claim -->`
marker in `docs/performance.md` quietly took a live claim out of its reach. The
paragraph is split; the marker now covers only the sentence that needs it.

## Why

`ledger/2026-09-29-mutating-the-real-tree-found-a-class-nothing-checked.md`
recorded that the previous entry's prediction — "at least one guard is green
for a reason that has nothing to do with what it checks" — had not come true in
the first six. It has now. `check_toolchain_pins.py` is precisely that: a guard
running green, in CI, on twenty-one jobs, against nothing but itself.

Its own docstring says what that costs. `setup-go@v6` pins `GOTOOLCHAIN=local`,
`protoc-gen-go-grpc@v1.6.2` declares `go >= 1.25.0`, the workflow installs 1.24,
and the Go client job went red with no Go changed. The guard was written so
that could not recur silently. It could.

The `check_cost_prose.py` finding is the other shape and worth keeping distinct:
no defect in the guard at all. The marker's granularity is a paragraph, and one
paragraph in `docs/performance.md` held both a measurement that needs excusing
and a restatement that does not. Excusing the paragraph excused both.

## Alternatives rejected

**Match `go install` and the pin on the same line, but allow a variable.** A
pattern like `go install <IDENT>` plus a lookup of `<IDENT>`'s assignment. That
is a Python parser, for one file, and it would not read the shell form at all.
Two independent patterns over the whole file is weaker — a file with an
unrelated `@v` string and an unrelated `go install ./...` would be counted — and
that is a false positive, which fails loudly. The old rule's failure was a false
negative, which did not.

**Teach `DECIDES` to skip comments with a real parser.** Three languages, and
Python is the only one where stripping `#` lines is not the whole story. It is
enough here, and where it is not — a docstring, which survives the strip — the
assignment requirement catches it. Both protections were mutated separately and
both were needed; a test written for one of them left the other's mutation
alive until the docstring case was added.

**Move the `<!-- not a cost-model claim -->` marker above the table instead.**
Then the paragraph is checked again and the table is excused. Rejected because
the table needs no excusing — removing the marker entirely leaves exactly one
complaint, and it is about the paragraph. Splitting is what puts the marker on
the sentence that earns it.

**Make the marker cover a sentence rather than a paragraph.** The right answer
in principle and a sentence splitter in practice, which is a parser this guard
has deliberately avoided being. A paragraph break costs one blank line and says
the same thing to a reader.

## Evidence

**The toolchain guard, before:** `python3 scripts/check_toolchain_pins.py`
reported `5 pinned go-version across 1 workflow(s), 2 pinned go install
target(s) in 2 file(s)`. `installers(ROOT)` returned
`{'scripts/check_toolchain_pins.py': [...], 'scripts/test_check_toolchain_pins.py': [...]}`
— the guard and its test, and nothing else.

**After:** `5 pinned go-version across 1 workflow(s), 2 pinned go install
target(s) in 1 file(s)`, and `installers(ROOT)` returns
`{'clients/go/scripts/generate_proto.py': ['google.golang.org/protobuf/cmd/protoc-gen-go@v1.36.12', 'google.golang.org/grpc/cmd/protoc-gen-go-grpc@v1.6.2']}`.

**The real-tree mutation, which is the whole finding.** Deleting
`"GOTOOLCHAIN": "auto"` from `generate_proto.py` and running the guard:

```
before:  ok    5 pinned `go-version` … every one of them deciding `GOTOOLCHAIN`
after:   clients/go/scripts/generate_proto.py runs a pinned `go install`
         (…protoc-gen-go@v1.36.12, …protoc-gen-go-grpc@v1.6.2) and never sets
         `GOTOOLCHAIN`, while .github/workflows/ci.yml pins `go-version`.
```

Recorded: `ledger/mutations/20260929T082330-clients-go-scripts-generate-proto-py.json`
(2 cases, one survivor — see below) and
`ledger/mutations/20260929T082350-clients-go-scripts-generate-proto-py.json`
(1 case, the survivor re-run with `expect_survivor` and its reason). The
survivor is dropping *one* of the two pins: one pin left is still a pinned
`go install`, the file still decides, and the answer is correctly unchanged.
What would be a finding is losing every pin, which is a two-place mutation
`mutate.py` does not express and which the fixture covers instead.

The guard's own rules: `ledger/mutations/20260929T082636-scripts-check-toolchain-pins-py.json`,
6 cases, **no survivors**. Two of the six were survivors on the first run and
each bought a case:

- restoring the bare-name `DECIDES` survived, because comment-stripping alone
  already caught the fixture. The case that tells them apart puts the mention
  in a **docstring**, which survives the strip — "we leave GOTOOLCHAIN alone"
  is the opposite of a decision;
- removing the self-skip survived, because no fixture had a file named
  `scripts/check_toolchain_pins.py` in it. Now one does, and a roster of
  exactly the guard and its test is a failing case rather than a passing run.

`python3 scripts/test_check_toolchain_pins.py` reports `14 passed, 0 failed`,
up from 10.

**The cost-prose finding.** `docs/performance.md` line 2050 reads
"1.02 requests per row, against `POINT_READ_COST = 1.0`" — a literal
restatement, exactly what `RESTATED` exists to check. Mutating `1.0` to `3.0`
and running the guard: `ok    25 prose claims about the cost constants, all
current`. Removing the marker entirely produces one complaint, about the
*other* sentence in the same paragraph ("the full scan returns 9,524–10,000
rows per request"), which is a measurement of this run and not an assertion
about `1 / SCAN_ROW_COST`. With the paragraph split the guard reports
**26** claims rather than 25, and the same mutation is refused:
`docs/performance.md:2046-2048 restates POINT_READ_COST as 3.0; it is 1.`

`sh scripts/check.sh` reports `83 passed, all of them`.

## What this does not do

**Eighteen guards still have no real-tree mutation.** Eight of twenty-seven
now. The running tally of what the survey has found: six clean, one gap between
guards (`check_cited_files.py`), one guard checking itself, one guard with a
mention-counts-as-a-use pattern, and one correct guard whose exemption marker
over-reached. Four findings in eight guards. That is a rate worth finishing the
survey for, and the remaining eighteen include every substantial one —
`check_handlers`, `check_write_paths`, `check_secret_types`,
`check_site_claims`.

**Nothing re-runs any real-tree mutation, and this entry adds more.** Unchanged
from two entries ago, and the pile grows with each one. `scripts/mutations.json`
plus a runner remains the fix.

**The self-skip is by filename.** `NOT_AN_INSTALLER` names two paths. A guard
renamed or a second example file added gets no warning — the skip does not
check that the file it names still carries an example, which is the same
roster-rot this repository checks for in `EXTERNAL`, `WITNESS` and `PROVES` and
does not check here.

**`PINNED` matches any `@v<digit>` token.** A file containing a `go install`
and an unrelated version-pinned string — an npm spec in a comment, a docker
tag — reads as a pinned Go installer. Nothing in the tree does today; comments
are stripped, which removes the most likely source.

**The cost-prose marker's granularity is unchanged.** One paragraph, all or
nothing. The fix here was to split a paragraph, not to change the rule, so the
next passage mixing an excusable measurement with a checkable restatement will
do the same thing silently. Nothing looks for a marker whose paragraph contains
a claim the guard *could* have checked — which is the mirror of the
"undeserved marker" rule that already exists, and is the one rule that would
have found this.
