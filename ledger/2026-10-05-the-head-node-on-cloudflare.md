# The explorer runs entirely on Cloudflare: a Worker, the head node in a Container, the data in R2

- **Date:** 2026-10-05
- **Author:** Claude Code (session: "Ideally it's all in cloudflare")
- **Touches:** `Dockerfile` (cross-compiling amd64 on an arm64 host),
  `examples/edge` (`container/`, `deploy.sh`, `wrangler.cloudflare.jsonc`,
  `src/worker.ts`, `package.json`, `README.md`), `docs/edge-client.md`,
  `.gitignore`
- **Kind:** feature

## What changed

The first deployment (`2026-10-04-the-worker-deployed.md`) put the Worker on
Cloudflare and kept the head node on the owner's machine, behind a quick
tunnel. Now the head node runs on Cloudflare too:
- `slate-serverd` runs as a **Cloudflare Container**, built from the
  repository's own `Dockerfile` with a derived `head.toml` on top;
- its store is an **R2 bucket**, through the existing S3 backend, with no new
  storage code;
- the Worker reaches it through a Durable Object binding (`getContainer`),
  not a URL.

`deploy.sh` does the whole thing from three inputs: the R2 credentials file,
the persona tokens, and the account id. Credentials go in as Worker secrets,
on stdin. The Worker passes them to the container as environment variables,
so the image holds nothing account-specific.

## Why

The owner asked whether it could run without the tunnel, then for it to be
"all in Cloudflare". The tunnel deployment stopped working whenever either of
two local processes stopped, and its URL changed on every restart. This also
tests something nothing else in the repository did: the S3 backend, and its
writer lease, against an object store that is not MinIO.

## Alternatives rejected

- **Keep the tunnel and make it named and permanent.** The head node would
  still be a process on a workstation, which is the dependency the owner
  asked to remove.
- **Rewrite the head node for the Workers runtime** (Durable Object storage in
  place of SlateDB). That is a second storage engine to keep in step, and the
  conformance runner exists because implementations of one thing disagree.
  The Container runs the same binary CI tests.
- **A separate Containers-only Dockerfile** that builds `slate-serverd` its
  own way. It would ship a binary the repository's image job does not check.
  `container/Dockerfile` is two lines on top of the real image. That needed
  the root `Dockerfile` to build `linux/amd64` on an arm64 host: Containers
  run amd64, and the owner's machine is arm64. The new branch installs a
  cross gcc only when `uname -m` is aarch64 and the target is amd64, so CI's
  amd64 builds take the old path unchanged.
- **Credentials in `head.toml`.** That would bake them into an image layer in
  Cloudflare's registry. `from_env = true` and `secret_env` keep them in
  Worker secrets.
- **More than one container instance.** The node holds a writer lease in the
  bucket, so a second instance would fight the first for it.
  `max_instances = 1`.
- **Creating the R2 token from the terminal.** I tried. Wrangler's OAuth
  session cannot create API tokens: the API answers error 9109. The owner
  created it in the dashboard.

## Evidence

**R2 behaves as the S3 backend expects.** Before deploying, I ran the
container's `head.toml` locally against the bucket, in trusted-header mode on
loopback, and seeded it:
- the first start took leadership through a lease in the bucket;
- `SIGTERM` resigned it;
- a restart took the lease at generation 2 and read back 5 authors, 11 books
  and 11 sales.

**The image.** It built as `linux/amd64` on the arm64 host, 43,593,693 bytes.
`wrangler deploy` pushed it to Cloudflare's registry, created the container
application `slate-explorer-edge-headnode`, and published Worker version
`cefa1004`.

**Live, with the tunnel and every local slate process stopped,** at
`https://slate-explorer-edge.jacobbeck-dev.workers.dev`:
- `/api/meta` reported `leader: true`;
- `app` saw 11 books, and `reader` saw 9 under its row policy;
- `analyst` got `born` as null, and a filter on `born` was refused;
- `stranger` was refused.

**The conformance runner against the live URL,** with the Python, Go and Node
adapters local on a freshly seeded node, agreed on 142 of 143 cases. The one
disagreement is a single cell: the soft-delete timestamp of the row retired
during seeding (`1791139447` live, `1791139573` local), which records when
each database was seeded. The runner's must-differ pair then reports that
case as not comparable, which is the second line of the same failure, not a
second finding. Cloudflare's browser-integrity check refuses Python's default
`urllib` user agent (error 1010). For this run only, the runner sent another
user agent; that change was not committed.

**No secret is in the tree.** Before committing, I searched the diff and the
untracked files for every value in the R2 credentials file and the tokens
file.

## What this does not do

- **The container sleeps after ten idle minutes.** The next request pays a
  cold start, which takes the lease again. I did not measure the cold start.
- **Anyone who can reach the Worker can act as any persona**, as in the first
  deployment. It is the same decision, and the data is demo data in a
  dedicated bucket.
- **Nothing tests the deployment.** It needs the owner's account, so it is
  not in CI. `wrangler.toml` and `wrangler dev` remain what CI runs.
- **A failover between two containers is not exercised.** The lease handover
  was tested by stopping and restarting one process locally against R2. It
  was not tested by Cloudflare replacing an instance.
- **The credentials live outside the repository.** The R2 keys are in
  `~/.config/slate/r2.env` on the owner's machine (mode 600) and in the
  Worker's secrets. The R2 token is scoped to the one bucket. The persona
  tokens are in the Worker's secret and in this session's scratch directory,
  which does not outlive the session. A redeploy can mint new tokens, because
  `deploy.sh` sets the node's tokens and the Worker's from the same file.
