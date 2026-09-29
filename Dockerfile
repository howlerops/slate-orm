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

FROM rust:1-bookworm AS build
WORKDIR /src

# The manifests first, so a source-only change does not re-resolve the
# dependency graph. `--locked` because an image built from a floating lockfile
# is an image nobody can rebuild.
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY clients/python/testserver clients/python/testserver

RUN cargo build --release --locked -p slate-serverd --bin slate-serverd \
    && strip target/release/slate-serverd

FROM gcr.io/distroless/cc-debian12

COPY --from=build /src/target/release/slate-serverd /usr/local/bin/slate-serverd

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
