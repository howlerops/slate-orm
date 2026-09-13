# Point the README's map at the territory

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5)
- **Touches:** `README.md`
- **Kind:** documentation

## What changed

The Layout section's "outside the workspace" note listed only the Python
client. It now lists all three clients, the explorer demo, and the site, as a
table matching the crate table above it.

## Why

That paragraph was written when Python was the only client. Since then two more
clients, a demo and a site have landed, and the section that exists to tell a
reader what is in the repository was telling them about a third of it.

Stale by omission is still stale: someone reading the map concludes the
territory is smaller than it is.

## Alternatives rejected

**Leaving it to the directory listing.** A reader who clones the repository can
see the directories; what they cannot see is which ones matter and why. The
table says what each *is*, which is the part a listing does not carry.

**A paragraph rather than a table.** The crate list directly above is a table,
and switching format halfway down the section is a small, free way to make
something look less considered than it is.

**Adding the conformance runner as one more table row.** It got a paragraph
instead, because its value is a claim rather than a location — every client's
own suite runs against the same server, which catches one client being wrong
and cannot catch two being wrong identically. A row saying "conformance tests"
does not say that.

## Evidence

941 Rust tests, 138 Python, 30 Go, 39 TypeScript, and 31 conformance cases
across the three adapters — all passing at this commit. `cargo fmt`, `cargo
clippy --workspace --all-targets`, `go vet`, `gofmt`, `tsc --strict` and `ruff`
clean.

Every link in the new table was checked against a path that exists.

## What this does not do

Nothing keeps this table current. It went stale once by accretion and will
again; the only defence is that it is short enough that adding a row is
obvious.

The README's Status section still describes the project as of before today's
work in places. This commit fixed the map, not the whole document.
