# Schema checks in Go and TypeScript, pinned against a third port

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `clients/go/slate/{schema,client}.go` and its tests,
  `clients/typescript/src/{schema,client,index}.ts` and its tests, three READMEs
- **Kind:** feature

## What changed

`TableDef`, a fingerprint, and `Declaring(...)` / `.declaring(...)` in both
clients. A request naming a declared table carries a claim the server checks;
one naming an undeclared table is unchanged.

## Why

Both clients shipped without it and both READMEs said so. The gap is worse than
"a missing feature" because of what these clients are: **everything is
ordinals**. Declaring `{id, title, year}` for a table that is really
`{id, year, title}` produces a client that reads titles as years, on every row,
forever, with nothing anywhere saying so. The Python client has had the check
since it was written; Go and TypeScript were one transposed column away from
silent corruption.

## Alternatives rejected

**Resolving column names from the declaration.** The obvious next step once a
client holds a schema, and still refused for the reason the clients were built
on: the wire carries ordinals, and a client that resolves names holds a second
copy of the schema that can *disagree* with the server's. A declaration used
only to check is a declaration that cannot silently be wrong.

**Making declarations mandatory.** Would make the check unmissable and break
every existing caller for a feature that is a safety net rather than a
requirement. Per-table and opt-in.

**Hashing nullability, defaults, checks, foreign keys or indexes.** They address
no column and a client cannot be wrong about them. Hashing them would make an
unrelated migration break every client — the failure mode that makes a
fingerprint worse than none. The Rust implementation says this and both ports
repeat it.

**Hashing a key that names a missing column as ordinal 0.** The obvious
implementation, and it makes `primaryKey: ["nope"]` hash identically to
`primaryKey: ["id"]` — a broken declaration the server accepts. Both ports hash
it as `?`, and both have a test.

## Evidence

30 Go tests and 47 TypeScript tests pass; `go vet`, `gofmt` and `tsc --strict`
clean. Nine mutations, all killed: not hashing the ordinal, the column type, or
the key's missing-column marker; and dropping the claim from an insert.

**The fingerprint is pinned against the Python port, not against itself.** A
port whose hash is internally consistent and differs from the server's refuses
*every* request, and the behavioural tests cannot see that — they assert that a
wrong declaration is refused, which a wrong hash also does. Only a value
computed by a different implementation catches it.

That mattered. The TypeScript port's length prefix must be the UTF-8 **byte**
length, and `"𝕏".length` in JavaScript is 2 while its UTF-8 length is 4 — so a
`.length` there produces a fingerprint no server accepts, for exactly the tables
whose column names are not ASCII. My first test for it compared two TypeScript
fingerprints of equal UTF-16 length and **passed under the bug**: swapping the
prefix changed both hashes and they stayed different from each other. Pinning
against Python's value for the same table kills it.

## What this does not do

Only the five requests that carry a `SchemaCheck` field are checked: insert,
upsert, update, delete and get. A `Query`, `Join` or `Aggregate` carries its
claim on the `Query` message and neither client sets it there — so a read
against a misdeclared table is still answered positionally-wrong. That is the
larger half of the exposure and it is not closed here.

Neither client accepts a renamed column's previous spelling, which the server
does accept. A client declaring the old name is refused where the Python client
would be served.
