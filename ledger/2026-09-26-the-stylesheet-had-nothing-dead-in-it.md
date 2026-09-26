# A caveat said `site/style.css` "may" carry rules for elements nobody uses. Deriving the answer took one script; the answer is no, and the script now runs on every push so it stays no. Its own test found a bug in it on the first run.

- **Date:** 2026-09-26
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `scripts/check_site_css.py` (new), `scripts/test_check_site_css.py` (new), `scripts/check.sh`, `.github/workflows/ci.yml`, `docs/caveat-status.json`
- **Kind:** a guard, and batch five of the re-triage pass

## What changed

`scripts/check_site_css.py` reports any class or id selector in
`site/style.css` that no page or script names. Over this tree: **61 classes, 0
ids, none dead**, across 13 pages and scripts. Nine cases in
`scripts/test_check_site_css.py` drive it over written trees; both run in
`scripts/check.sh` (now 57 checks) and in CI. Batch five of the reading pass —
ten more caveats — is stamped alongside, nine still true.

## Why

**Because an admitted unknown is the caveat that never closes.**
`ledger/2026-09-16-the-site-looks-like-the-family-it-belongs-to.md` wrote that
the stylesheet "*may* no longer" match the markup, which is not a claim anyone
can refute by reading; it is a request for a derivation. Nobody derives it,
because deriving it is the whole work, so it sits open indefinitely looking
like a known gap when it is a question nobody asked.

That makes it a different kind from the five stale caveats found earlier today,
which were all "waits for X" where X had shipped
(`ledger/2026-09-26-batch-four-and-the-shape-that-keeps-recurring.md`). This one
was never true or false — it had no answer. The right close for it is not a
reading, it is a measurement plus something that keeps the measurement true,
which is why this is a guard and not a stamp.

The site has been rewritten twice — the dropdown panel became the workbench,
the landing page was restyled to match ironrain — and each time the markup moved
and the stylesheet did not have to. A dead CSS rule is invisible: nothing
renders wrong, nothing fails, and a reader treats the stylesheet as a
description of the page. That is the class of rot worth a guard.

## Alternatives rejected

**Close it by hand and move on.** A one-off `grep` answered it in a minute and
the answer would have been stale by the next site change, at which point the
caveat would have to be reopened by someone who first had to notice. The caveat
was open for ten days precisely because the answer decays.

**Parse the HTML and the JavaScript properly.** A real parse would find every
class actually attached to an element and could catch the *other* direction too
— a class the markup uses and the stylesheet does not define. It would also
refuse `` `<span class="badge" data-tone="${tone}">` `` and every other
constructed name, and each one would need an exception, and the exception list
would become the thing that goes stale. Substring matching is loose in the
direction that costs nothing (a false pass leaves a rule that would have stayed
anyway) and exact in the direction that matters (a name occurring *nowhere* is
unambiguously dead). The docstring says this and a test case pins it, so nobody
"tightens" it later without reading why.

**Catch the reverse direction as well** — markup using a class the stylesheet
does not define. Deliberately not: that is a styling bug a reader sees the first
time they load the page, so it does not need a guard. A dead rule is the one
that hides.

**Check whether a rule's properties are all overridden.** The genuinely
complete version, and it needs a renderer and a cascade. Out of proportion, and
recorded below.

## Evidence

**The guard's own test found a bug in the guard on its first run.** The id
pattern was `#([a-zA-Z][\w-]*)`, which reads `--line: #ddd` as an id named
`ddd` — a hex colour. Seven of the nine cases failed, including the one over a
deliberately clean fixture, which is the shape of failure that says the checker
is wrong rather than the tree. `selectors()` now strips the four CSS hex forms
first, and the comment explains which run found it.

Worth naming precisely, because the near-miss is the interesting part: the
throwaway `grep` that first answered this question used **the same pattern**
and reported a clean tree, because this stylesheet's one hex colour is
`#6ddc9a` and `6` is not `[a-zA-Z]`. The ad-hoc check was wrong and passed; the
guard was wrong and failed, because it had a fixture written to be clean. That
is the entire argument for testing a check against a written tree rather than
against this repository.

**Mutations: six run, six caught**, each by a named case.

| mutation | caught by |
|---|---|
| the comment stripper dropped | 4 cases, incl. `a class named only inside a CSS comment is not treated as defined` |
| the hex-colour stripper dropped | 4 cases, incl. `a stylesheet whose every selector is used passes` |
| `GENERATED` emptied, so `slate_wasm.js` counts as a source | `the generated wasm glue is not a source` |
| `site/docs/*.html` dropped from the sources | `the docs pages count as sources` |
| `dead` forced to `[]` | 3 cases |
| `orphan_ids` forced to `[]` | `an id no page or script names is reported` |

**`scripts/check.sh`: 57 passed, all of them** (55 before).

**Batch five, nine still true:**

| caveat | checked against |
|---|---|
| no composite relating key | no occurrence in the kernel or the daemon |
| the `Related` trait and the wire disagree | `crates/slate-orm/src/relation.rs:55` still takes any two ordinals via `local()`/`foreign()` |
| the subquery elision is untested by the Rust suite | `site/workbench.js:749` builds the marker; `site/check/workbench.py` has a subquery case and **zero** occurrences of the marker text |
| predicate writes are one table | no `UPDATE … FROM`, no predicate write across a join |
| the matched rows are held in memory at once | unchanged |
| nothing was seen | still no visual regression anywhere; `ledger/2026-09-20-the-undo-a-visitor-can-see.md:114` says the same of itself |
| no mobile navigation to speak of | two `@media` blocks in `site/style.css`, both re-stacking a grid; no menu, no disclosure |
| nothing changed on the server about cancellation | `config.rs:432` bounds a request's own run time; nothing tests that the server acts on a client-side deadline |
| no default deadline, so the hang is still reachable | `clients/python/src/slate/client.py:1540` — "`None` means no deadline, which is gRPC's default and this client's" |

**Rate.** Five closures over 48 reads today. The prior from the earlier
unbiased sample was 1 in 8; this is 1 in 9.6, which is the same number given
how few reads either rests on. Not revised.

**The runs themselves**, as `mutate.py` recorded them:

- `ledger/mutations/20260926T003442-scripts-check-site-css-py.json` — scripts/check_site_css.py

## What this does not do

**It cannot see a rule that is reachable and pointless.** A `.card` whose every
declaration is overridden by a later rule is dead in every sense but the one
this measures. That needs a cascade, which needs a renderer.

**It does not check the reverse direction**, deliberately — see above. A class
in the markup with no rule is a visible bug.

**The id check is vacuous on this tree.** `site/style.css` defines zero id
selectors, so that assertion currently proves nothing about the site; it is
exercised only by `test_check_site_css.py`. Stated rather than left to be
discovered, because "0 of 0 are orphaned" is exactly the shape of a check that
has quietly stopped applying.

**Substring matching accepts a name that only appears in a comment.** A class
mentioned in a `<!-- -->` or a `//` note counts as used. The docstring says so;
it is the same looseness that makes the dynamic case work and it cannot be had
one way without the other.

**`examples/explorer/web` has its own stylesheet and is not checked.** This is
`site/` only. The demo's CSS could be full of dead rules and this would pass.
