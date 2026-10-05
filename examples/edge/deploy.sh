#!/bin/sh
# Deploy the explorer Worker and its head node, both on Cloudflare: the Worker,
# a Container running slate-serverd, and the node's database in R2.
#
#     R2_ENV=~/.config/slate/r2.env TOKENS=tokens.json sh deploy.sh
#
# R2_ENV is a file with ACCESS_KEY_ID= and SECRET_ACCESS_KEY= lines (an R2 API
# token scoped to the bucket). TOKENS is a JSON object from persona to bearer
# token. Neither value is printed: both go to `wrangler secret put` on stdin.
# CLOUDFLARE_ACCOUNT_ID names the account whose R2 endpoint the node uses.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
: "${R2_ENV:?set R2_ENV to the file holding the R2 S3 keys}"
: "${TOKENS:?set TOKENS to the persona -> token JSON file}"
: "${CLOUDFLARE_ACCOUNT_ID:?set CLOUDFLARE_ACCOUNT_ID}"
export WRANGLER_SEND_METRICS=false

echo "deriving the container's config from examples/explorer/head.toml"
python3 "$here/container/derive.py"

# linux/amd64, which is what Cloudflare Containers run, whatever this machine is.
echo "building slate-serverd for linux/amd64"
(cd "$root" && docker buildx build --platform linux/amd64 -t slate-serverd:cloudflare --load .)

cd "$here"
value() { sed -n "s/^$1=//p" "$R2_ENV"; }
echo "setting secrets"
# `printf '%s'`, not the bare value: a trailing newline stored inside an S3
# secret breaks every request's signature, and says so only as `403`.
printf '%s' "$(value ACCESS_KEY_ID)" | npx --no-install wrangler secret put R2_ACCESS_KEY_ID -c wrangler.cloudflare.jsonc
printf '%s' "$(value SECRET_ACCESS_KEY)" | npx --no-install wrangler secret put R2_SECRET_ACCESS_KEY -c wrangler.cloudflare.jsonc
npx --no-install wrangler secret put SLATE_TOKENS -c wrangler.cloudflare.jsonc < "$TOKENS"

echo "deploying"
npx --no-install wrangler deploy -c wrangler.cloudflare.jsonc \
  --var "R2_ENDPOINT:https://$CLOUDFLARE_ACCOUNT_ID.r2.cloudflarestorage.com"
