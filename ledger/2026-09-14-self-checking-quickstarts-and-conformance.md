# The quickstarts and the conformance suite now check themselves

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `site/check/quickstarts.py` (new), `site/index.html`,
  `site/README.md`, `examples/explorer/run.sh`,
  `examples/explorer/conformance/conformance.py`,
  `examples/explorer/README.md`, `crates/slate-kernel/src/read.rs`
- **Kind:** repair

## What changed

Two things that had been run once, by hand, are now one command each.

`python3 site/check/quickstarts.py` pulls the four `<pre><code>` panels out of
the landing page, validates the TOML panel with `slate-serverd --check`, starts
a node from it, and runs the Python, Go and TypeScript snippets against that
node. Each must insert a row and read it back.

`./run.sh --conformance` starts the head node and all three adapters on ports
the kernel picked, runs the 31-case conformance suite against them, tears the
stack down, and exits with the suite's status.

## Why

The site's README said, in as many words, that the snippets were "**not**
checked automatically" and that wiring it up "is worth doing and is not done".
Example code that does not run is the most embarrassing kind of stale
documentation, because it is the first thing a reader tries.

The conformance suite had the same shape of problem for a different reason: it
required a stack somebody had already started in another terminal, on three
fixed ports. A suite that needs a two-terminal ritual is a suite that runs when
somebody remembers, which for a cross-SDK differential is exactly wrong — the
divergences it catches arrive one client change at a time.

## What it found

**The TOML on the landing page does not parse.** It showed

```toml
[storage]
backend = "s3"
bucket = "my-bucket"
```

and `bucket` is not a `[storage]` field — it lives under `[storage.s3]`, whose
struct is `deny_unknown_fields`, so a node started from the page's config
refuses with a parse error. Anyone copying the quickstart hit it on their first
command.

It had been there since the page was written, one commit after a README
claiming the snippets had been extracted and executed. Both were true of the
three *code* snippets. Nobody had run the TOML, and nothing distinguished the
one panel that had not been checked from the three that had. That is the whole
argument for the check: a claim about four things, verified for three.

Fixed, and while fixing it the page now shows `access_key_id_env` rather than a
key, which is the more useful thing for a quickstart to teach.

## Alternatives rejected

**Keeping the snippets in files and including them into the page.** The usual
answer, and the better one for a site with a build step. This site deliberately
has none — two static pages, no generator, no toolchain — and adding one to
solve four code blocks inverts that trade. Extraction runs the page's own text,
which is also strictly the stronger property: an include can drift in the
direction of the file being right and the page being stale, and extraction
cannot.

**Running the snippets verbatim, with no substitution at all.** Would need the
page to name a port that is free, which no fixed port is, and an S3 bucket that
exists. Instead each rewrite is asserted: the port substitution must fire
exactly once per snippet, and the storage swap is done by splitting the file so
everything outside `[storage]` is the page's text by construction rather than
by a regex that was eyeballed once.

**A lowest-common-denominator harness for the three languages.** Rejected: the
Go panel is a fragment on purpose (a reader pastes it into a function they
have), Python's is a whole program, and TypeScript's is a module with top-level
`await`. One harness that ran all three the same way would have to run none of
them the way a reader would. Three wrappers, each owning only the boilerplate
its ecosystem demands, with the snippet spliced in between markers.

**Fixed ports for the conformance mode, matching the demo.** They collide with
a demo stack that is already up, and the collision presents as an unreachable
adapter — which reads exactly like the SDKs disagreeing. Free ports; the
interactive modes keep the fixed ones, because a human reads those off the
terminal and the frontend is configured with them.

## Evidence

`python3 site/check/quickstarts.py`: the TOML validates, a node starts from it,
and all three snippets insert and read back. Mutated five ways, each killed by
a *named* case:

| mutation | what failed |
| --- | --- |
| Python's filter changed to `ge(3000)` | `the python snippet ran but never printed 'A Wizard of Earthsea'` |
| Go's `Table: "books"` → `"nope"` | `the go snippet exited 1` |
| TypeScript's `table: "books"` → `"nope"` | `the typescript snippet exited 1` |
| the TOML's grant narrowed to `["read"]` | `the python snippet exited 1` |
| Python's snippet no longer names `127.0.0.1:7421` | `mentions 127.0.0.1:7421 0 times, expected once` |

The fourth is the one worth noting: it is a mutation of the *config*, and it is
caught through a snippet, which is the only evidence that the two panels are
being checked against each other rather than separately.

`./run.sh --conformance`: `31 cases: the three SDKs agree on all of them`,
having built, seeded and started four processes and stopped them again.
Mutated by making the Python adapter reverse its sort keys — `ordered by a
float (app): the adapters disagree`, exit 1, with the status propagating out
through `run.sh` (which needs the `trap` and therefore cannot `exec`).

Also fixed an `unused_mut` warning left in `read.rs` by the grouped-chain
narrowing two commits ago.

## What this does not do

Neither runs on a schedule. There is no CI in this repository to wire them
into; both are one command now, which is the part that was missing, but "one
command somebody has to think to run" is still a person in the loop.

The quickstart check does not verify the *prose* around the snippets — the
install lines (`pip install slate-client`, `npm install @slate-orm/client`)
name packages that are not published anywhere, and nothing here notices.

The conformance mode picks free ports for the adapters but the frontend still
hard-codes the demo's three, so `--conformance` and the browser UI cannot share
a stack. That is the next task's problem. *(Closed by
`2026-09-14-frontend-tests-and-configurable-ports.md`: the frontend reads
`VITE_GO_URL` and friends, which `run.sh` exports.)*

The TypeScript snippet is compiled with a `tsconfig` this checker writes, not
the one a reader would have. It is strict and NodeNext, which is the shape that
matters, but a reader on a looser config could hit something this does not.
