# The head node for the all-Cloudflare deployment: the repository's own
# image (built by `deploy.sh` from the root Dockerfile, for linux/amd64, which
# is what Cloudflare Containers run), plus a configuration file.
#
# The configuration is derived from `examples/explorer/head.toml` by
# `deploy.sh` — the same tables, grants and policies the conformance runner
# exercises — and holds nothing account-specific: the bucket, endpoint and
# keys arrive as `SLATE_S3_*` environment variables, and the bearer tokens as
# `SLATE_TOKEN_*`, all from the Worker's secrets.
ARG BASE=slate-serverd:cloudflare
FROM ${BASE}
COPY head.toml /etc/slate/head.toml
CMD ["--config", "/etc/slate/head.toml"]
