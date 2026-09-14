# Not making every reader pay for the playground

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `site/{index.html,playground.js,style.css}`, `site/check/playground.py`
- **Kind:** fix

## What changed

The wasm bundle is no longer fetched on page load. A **Load the database**
button starts it, and an `IntersectionObserver` starts it automatically when
the panel comes within 200px of the viewport.

The browser check now records every request and asserts that **no `.wasm` is
requested before the reader asks** — and that one is afterwards.

## Why

596 KB gzipped was landing on every visitor to the landing page, including the
large majority who read the first two paragraphs and leave. A playground is
worth that to somebody who uses it and worth nothing to somebody who does not,
and the page cannot know which until they scroll.

## Alternatives rejected

**A button alone.** Honest and slightly hostile: a reader who scrolls to a
panel headed "Run a query" has already asked, and making them click again is
ceremony. The observer covers that; the button covers readers who arrive via
`#playground`, have no `IntersectionObserver`, or want to start the fetch
before scrolling.

**`IntersectionObserver` alone.** Leaves no affordance when the API is absent
and nothing to look at while 600 KB arrives. The button is also where the
failure goes: "Loading failed — try again", which is better than a panel that
never appears.

**`<link rel="prefetch">` at low priority.** Still fetches for everyone, just
more politely, and on a metered connection "politely" is not the axis that
matters.

**Trusting the code.** The check asserts the *observed* request list rather
than reading the source, which is the difference between "we wrote it to be
lazy" and "it is lazy". The mutation below is why that distinction earned its
keep.

## Evidence

Nine assertions in `site/check/playground.py`, two new: the wasm is not fetched
before the button is clicked, and it is fetched once it is.

Mutation: calling `start()` at the end of setup — eager loading, exactly what
this commit removes — fails with `1 wasm request(s) before the button was
clicked`.

Getting that message took a second pass. The first version of the mutation
failed with a Playwright timeout about element stability, because eager loading
disables the button and the driver was clicking unconditionally. A check that
detects the right problem and reports the wrong cause is only half a check, so
the driver now clicks only an enabled button and lets the request count speak.

## What this does not do

The observer's 200px margin is a guess at "about to be visible", not a measured
one. Too small and the reader waits after arriving; too large and a reader who
scrolls past quickly still pays. Nothing measures which happens.

A reader who loads the page and immediately jumps to `#playground` triggers the
observer at once, so they pay the full 600 KB before seeing anything — the
deferral helps the scroller, not the deep-linker.

There is no progress indication beyond "Loading…". The fetch is a single
`import()` and wiring a progress bar would mean fetching the bytes by hand and
instantiating them separately, which is a lot of machinery for a bar.

Nothing caches across visits beyond what the browser's HTTP cache does on its
own, which for a Pages-served immutable-ish asset is probably enough and has
not been checked.
