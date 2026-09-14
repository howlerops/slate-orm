# The demo frontend gets tests, and the demo gets configurable ports

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `examples/explorer/web/src/api.ts`,
  `examples/explorer/web/{test,e2e}/` (new), `examples/explorer/web/package.json`,
  `examples/explorer/web/tsconfig.test.json` (new),
  `examples/explorer/run.sh`, `examples/explorer/README.md`, `.gitignore`
- **Kind:** feature

## What changed

The explorer's frontend had no tests and three hard-coded `localhost` ports. It
now has both kinds of test and no hard-coded ports.

* `cd web && npm test` — 12 cases over `src/api.ts`, nothing started, ~150 ms.
* `./run.sh --e2e` — 14 cases driving the five panels in Chromium against a
  head node, three adapters and a vite server that the script starts on free
  ports and stops again.
* Adapter URLs come from `VITE_GO_URL`, `VITE_NODE_URL`, `VITE_PYTHON_URL`,
  defaulting to the demo's fixed ports, and `run.sh` exports whichever it used.

## Why

The panels had been driven in a browser exactly once, by hand, when they were
written. Everything since — three SDK changes, a schema check, a grouped chain
— had been verified through the adapters and never through the UI.

The ports mattered for a smaller and more concrete reason: `--conformance`,
added an hour earlier, picks free ports so it can run beside a demo stack, and
the frontend could not follow it there. Two halves of one demo disagreeing
about where the backend is.

## The mutation that mattered

`the rows panel renders through the node client`, asserting on the rendered
table, **passed with the SDK switch disconnected.** Pinning the panel's query
to `api.query("go", ...)` left all 14 cases green.

It is obvious in hindsight and it is the demo's central claim. All three
adapters answer identically — that is the entire point — so a table rendered by
the wrong client is indistinguishable from one rendered by the right one. An
assertion about the rows can never see the difference.

The check now watches `page.on("request")` and asserts the origin the panel
actually asked, against the URL `run.sh` handed vite. Selecting `node` must
produce a request to the node adapter and to neither of the others. With that,
the mutation fails on two cases, and so does a second mutation that makes
`adaptersFrom` ignore the environment — which is the only evidence that the new
port plumbing is live rather than merely written.

This is the fourth time in this repository a test has been satisfied by the bug
it was meant to catch. The pattern each time: the assertion was about an
*outcome* that two different mechanisms produce identically.

## Alternatives rejected

**`@playwright/test` instead of the `playwright` library.** The stack's
lifecycle belongs to `run.sh` — it starts four processes and a vite server and
has the trap that stops them. A test runner underneath that owns nothing but
its own config file, and `webServer` in a Playwright config would duplicate the
half of `run.sh` that is already there and already used by `--conformance`.

**A component test with jsdom and `@solidjs/testing-library`.** Would cover the
panels without a browser, faster, and would not have caught the mutation above:
the SDK switch is a claim about which of three processes is asked, and a jsdom
test mocks the fetch that carries the answer. Real browser, real adapters, or
the central case is untested.

**Leaving the schema assertion as a literal.** The first version compared
`TABLES` against a second copy of the same array, which no drift can break. It
now parses `head.toml` — thirty lines of regex, which throws if it finds no
tables so that a broken parser fails rather than agreeing with an empty
expectation.

**An `.env` file for the frontend's URLs.** Exported variables instead: a run
on free ports would otherwise leave a stale `.env` naming ports nothing is
listening on, and the next `npm run dev` would silently use it.

## Evidence

`npm test`: 12 passed. Four mutations of `src/api.ts`, each killed by a named
case — a blank URL no longer falling back (2 cases), `bytes` losing its `0x`,
the identity header pinned to `app`, and refusals parsed as results.

`./run.sh --e2e`: 14 passed, having built and started five processes. Five
mutations:

| mutation | result |
| --- | --- |
| the identity header pinned to `app` | 3 failures (the policy, the plan refusal, the stranger) |
| a refusal rendered as a result | the same 3 |
| the rows panel's client pinned to `"go"` | 2 failures — **only after the request-origin check; 0 before** |
| `adaptersFrom` ignoring `VITE_GO_URL` | `the rows panel asks the go adapter, and only it` |
| `having` dropped from the aggregate request | `HAVING removes groups, and removes the smallest ones` |

One finding that was not a bug: `a reader sees fewer books than the app` failed
on the first run, and the app was right. The panel opens on `year >= 1960`,
which is the row policy's own predicate, so under the default filter the two
identities legitimately agree. The check clears the filter first, and now
requires the fixture to contain a pre-1960 book before concluding anything —
otherwise it would pass for want of data.

## What this does not do

~~Neither suite runs on a schedule; the repository has no CI.~~ **Wrong: it
had one, which had never fired.** Both run in CI now, and `--e2e` found a
portability bug there that no local run could — see
`2026-09-14-ci-that-had-never-run.md`. `npm test` is still fast enough to run
on every change; `--e2e` takes about two minutes.

The e2e covers the five panels' central behaviour, not their controls
exhaustively: the `like`/`ilike` operators, the table switcher, the projection,
the offset and both sort directions are exercised through the conformance
suite's HTTP cases but never clicked.

The bar chart is asserted on count and threshold, not on geometry. A CSS change
that renders every bar at zero width would pass.

`web/package.json` now depends on `playwright` for a demo, which is a
heavyweight devDependency for an example directory. Justified only because the
browser is already present in this environment; a reader without one gets a
download on first `npm install`.
