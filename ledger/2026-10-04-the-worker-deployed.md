# The edge Worker deployed to Cloudflare, behind token authentication

- **Date:** 2026-10-04
- **Author:** Claude Code (session: "Cloudflare should be authed for you")
- **Touches:** `examples/edge/src/worker.ts`, `examples/edge/README.md`
- **Kind:** feature

## What changed

The Worker gained bearer-token support. A Wrangler secret, `SLATE_TOKENS`, maps
each persona to a token, and the Worker sends the persona's token as
`authorization: Bearer` through the `fetch` option `Client.connectWeb` already
takes. No client change was needed. The README's deploy section, which said
"untested", now gives the steps that worked.

The Worker is deployed to the owner's Cloudflare account as
`slate-explorer-edge`. It reaches a head node on the owner's machine through a
quick tunnel (`cloudflared tunnel --url`), and that node runs in `[auth] mode =
"token"`.

## Why

The owner asked for the deploy once wrangler was authenticated. The README's
quick path would have exposed the explorer's head node in `trusted-header`
mode, which trusts whatever identity a caller claims. The tunnel URL is
public, so anyone with it could have acted as `app` against a process on the
owner's machine. Token auth makes the tunnel refuse anything without one of
four tokens, and only the Worker holds them.

## Alternatives rejected

- **`trusted-header` behind the tunnel.** One line of config, and it is the
  exposure above.
- **A named tunnel, or Cloudflare Access in front of it.** These give a stable
  hostname and an identity gate, but they need DNS and Zero Trust
  configuration on the account, which changes more of the owner's setup than
  a test deployment warrants. A quick tunnel lasts as long as its process,
  which suits this.
- **Teaching the client an `extra` headers field**, as the Python client has.
  The `fetch` option already carries one, so the Worker could add the header
  itself without a new client API.

## Evidence

- **Head node, locally.** With no token: `grpc-status: 16`, "no
  `authorization` in the request metadata". With a wrong token:
  `grpc-status: 16`, "the bearer token is not one this server accepts". With
  the reader's token: data, then `grpc-status:0`.
- **Through the tunnel, no token:** `grpc-status: 16`.
- **Deploy:** 829.62 KiB uploaded (97.09 KiB gzip), Worker startup 15 ms.
- **The live Worker:**
  - `/api/meta` answers `leader: true`, which it learns from the head node,
    through the tunnel, with a token.
  - The analyst reads `authors` with `born` null, and is refused a filter on
    `born` with `COLUMNS_WITHHELD`.
  - The reader's earliest book is from 1961, so the `year >= 1960` policy holds.
  - The stranger is refused with `ACCESS_DENIED`.
- **The conformance runner against the live URL**, with the edge adapter
  pointed at it and the other three running locally: 142 of 143 cases agree.
  The 143rd differs only in the soft-delete timestamp of a row retired during
  seeding: `1791139447` against `1791139573`. The live Worker's database was
  seeded about two minutes before the local stack's, so this is data, not the
  client.
- **Error 1010.** The first live run died on Cloudflare's browser-integrity
  check, which refuses Python's default `urllib` user agent at the edge before
  the Worker runs. A different user agent, set when invoking the runner and
  not committed, got past it.

## What this does not do

- **The deployment lasts as long as two processes on the owner's machine:**
  the head node and the tunnel. When either stops, the Worker returns
  errors. The tunnel's URL changes on every restart.
- **Anyone who can reach the Worker can act as any persona**, including `app`,
  which writes. That is the explorer's design: the `x-demo-identity` switch
  is the demo. The writes are the adapter's fixed operations against a seeded
  demo store in a scratch directory, not arbitrary requests.
- **The tokens live in the session's scratch directory and in the Worker's
  secret.** Neither is in the repository.
