# The head node, as an image.
#
# `slate-serverd` is one static-ish binary, a TOML file and a bucket. That is
# the whole deployment, and it is why this file is short — there is no runtime
# to install, no entrypoint script deciding things, and nothing to configure
# except the config the operator brings.
#
# # Why a distroless base and not `scratch`
#
# The binary links glibc, because the release targets are `*-linux-gnu` and
# nothing here has been built or tested against musl. `scratch` would need a
# static build this repository does not produce; `debian:slim` would ship a
# shell and a package manager to a container whose only job is to listen on one
# port. `gcr.io/distroless/cc-debian12` is the glibc runtime and its CA
# certificates and nothing else — the certificates matter, because the object
# store is reached over HTTPS and a container without them fails at the first
# `PutObject` with an error about a certificate rather than about a bucket.
#
# # Why the build stage pins nothing
#
# `rust:1-bookworm` rather than a pinned patch: the version that matters is the
# one in `rust-toolchain`-adjacent CI, and a second pin here would be a second
# thing to bump and a second way for the two to disagree. What the image
# promises is the *binary*, and `scripts/check_versions.py` is what keeps its
# version honest.
#
# # Why the builder is pinned to the *build* platform
#
# `--platform=$BUILDPLATFORM` means this stage always runs natively and
# cross-compiles, rather than running under emulation once per target. That is
# not a micro-optimisation. Measured on `v0.1.0`, which built the obvious way:
#
#     the whole release, minus the image      2m27s
#     the image, linux/amd64 + linux/arm64   76m36s
#
# One `buildx` step, and for 76 of those minutes the arm64 half was running
# `rustc` through QEMU. The comparison that makes it damning is in the same
# run: `release-build.yml` produced the *aarch64 binary* in 2m02s, because it
# cross-compiles on an amd64 host. Same code, same profile, ~37x.
#
# `ledger/2026-10-02-the-release-that-shipped-five-things.md` has the numbers
# and why they were not acted on that day.

FROM --platform=$BUILDPLATFORM rust:1-bookworm AS build
WORKDIR /src

# Supplied by buildx, one value per entry in `platforms:`. Declared here rather
# than used inline so a build invoked by plain `docker build`, which sets
# neither, fails on the empty case below instead of silently producing a
# host-architecture binary under an arm64 tag.
ARG TARGETARCH

# The manifests first, so a source-only change does not re-resolve the
# dependency graph. `--locked` because an image built from a floating lockfile
# is an image nobody can rebuild.
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY clients/python/testserver clients/python/testserver
# And every other workspace member, manifest included. `cargo` resolves the
# *whole* workspace before it builds one package, so a member this image does
# not copy is `failed to load manifest for workspace member` and a red job on
# a change that touched nothing in `crates/`. That is not hypothetical: adding
# `examples/helpdesk` to `members` turned this job red while the other
# twenty-two stayed green, because it is the only one that builds from a
# partial copy of the tree. `scripts/check_workspace.py` now holds this list
# to `members` so the next one is caught before the push.
COPY examples/helpdesk examples/helpdesk

# The cross-compile. Three things here are load-bearing and none is obvious.
#
# **The C toolchain, because four dependencies are not Rust.** `aws-lc-sys`,
# `ring`, `lz4-sys` and `zstd-sys` compile C through the `cc` crate, and
# `aws-lc-sys` additionally drives `cmake`. `cargo` cross-compiles Rust from
# the target triple alone; a C build script does not, and will happily invoke
# the *host* compiler and produce x86-64 objects that fail at link with
# `incompatible with aarch64`. `CC_<triple>` and `CXX_<triple>` are what the
# `cc` crate reads, `AR_<triple>` what it archives with, and
# `CARGO_TARGET_<TRIPLE>_LINKER` what cargo links the final binary with. All
# four are needed; three of them produce a different, later, less obvious
# failure when missing.
#
# The triple is spelled with **underscores** in those names, and that is not a
# style choice. `cc` accepts either spelling, but `CC_aarch64-unknown-linux-gnu`
# is not a valid shell identifier, so `export` rejects it outright in `dash` —
# which is what `/bin/sh` is on this base, and therefore what `RUN` runs.
#
# **`strip` is also a cross tool.** The host `strip` does not know aarch64 and
# says so only after a successful build, which is the worst place to find out.
# `binutils-aarch64-linux-gnu` arrives with the gcc package and carries it.
#
# **The output path moves.** `--target` puts the binary under
# `target/<triple>/release/`, so the final stage can no longer name a fixed
# path. It is copied to `/out/` here and the runtime stage reads that, which
# also means the runtime stage does not have to know the triple.
# **And the other direction, on an arm64 host.** CI's runners are amd64, so
# building for amd64 was always native and needed no cross tools. A build on
# an arm64 workstation for an amd64 target — Cloudflare Containers run amd64,
# and `examples/edge/deploy.sh` builds this image on whatever machine deploys
# — has the same problem the arm64 branch solves, mirrored: the host linker
# cannot link x86-64, and fails only at the end. So when the build stage is
# itself aarch64, the amd64 branch installs the x86-64 cross toolchain the
# same way. On an amd64 host nothing changes.
RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) \
        triple=x86_64-unknown-linux-gnu; prefix=; \
        if [ "$(uname -m)" = aarch64 ]; then \
          prefix=x86_64-linux-gnu-; \
          apt-get update; \
          apt-get install -y --no-install-recommends \
            gcc-x86-64-linux-gnu g++-x86-64-linux-gnu cmake; \
          rm -rf /var/lib/apt/lists/*; \
        fi ;; \
      arm64) \
        triple=aarch64-unknown-linux-gnu; prefix=aarch64-linux-gnu-; \
        apt-get update; \
        apt-get install -y --no-install-recommends \
          gcc-aarch64-linux-gnu g++-aarch64-linux-gnu cmake; \
        rm -rf /var/lib/apt/lists/* ;; \
      *) \
        echo "TARGETARCH=${TARGETARCH:-<unset>} is not one this image builds;" \
             "buildx sets it, plain \`docker build\` does not" >&2; \
        exit 1 ;; \
    esac; \
    rustup target add "$triple"; \
    under="$(echo "$triple" | tr '-' '_')"; \
    upper="$(echo "$under" | tr 'a-z' 'A-Z')"; \
    if [ -n "$prefix" ]; then \
      export "CC_${under}=${prefix}gcc" \
             "CXX_${under}=${prefix}g++" \
             "AR_${under}=${prefix}ar" \
             "CARGO_TARGET_${upper}_LINKER=${prefix}gcc"; \
    fi; \
    cargo build --release --locked --target "$triple" \
      -p slate-serverd --bin slate-serverd; \
    "${prefix}strip" "target/${triple}/release/slate-serverd"; \
    mkdir -p /out; \
    cp "target/${triple}/release/slate-serverd" /out/slate-serverd

FROM gcr.io/distroless/cc-debian12

COPY --from=build /out/slate-serverd /usr/local/bin/slate-serverd

# 7421 is what `examples/explorer/head.toml` and the quickstart both use, so a
# reader who has run the demo finds the same number here. It is a default and
# not a rule: the config decides, and `EXPOSE` documents rather than binds.
EXPOSE 7421

# No `CMD`, only an entrypoint: `slate-serverd` requires `--config`, so an
# image with a default command would either invent a config path that is not
# there or start a server nobody configured. `docker run … --config /etc/slate/head.toml`
# is the whole interface, and the error from running it with no arguments is
# the binary's own usage message.
ENTRYPOINT ["/usr/local/bin/slate-serverd"]
