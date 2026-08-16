# Container image for collapse-api.
#
# Build from the REPOSITORY ROOT, not from this directory: collapse-api depends
# on collapse-core through a path dependency, so the whole workspace has to be
# in the build context.
#
#   docker build -f apps/api/Dockerfile -t collapse-api:dev .
#
# Bump both image tags below together when moving to a newer Debian release.

# --- build -------------------------------------------------------------------
FROM rust:1.90-slim-bookworm AS builder

WORKDIR /app

# Only the workspace members are copied, so editing the desktop app, the landing
# or the docs never invalidates this layer. All three members must be present:
# cargo refuses to load a workspace whose members are missing, even when
# building just one of them.
COPY Cargo.toml Cargo.lock ./
COPY apps/core apps/core
COPY apps/cli apps/cli
COPY apps/api apps/api

# --locked matches the release pipeline: a drifted Cargo.lock fails the build
# instead of being silently re-resolved here.
#
# The cache mounts survive between builds, which is what makes a rebuild after a
# source edit take seconds. They are NOT part of the image layer, so the binary
# has to be copied out of the mounted target directory inside this same RUN;
# a later "COPY --from=builder /app/target/..." would find nothing.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    cargo build -p collapse-api --release --locked \
    && cp /app/target/release/collapse-api /usr/local/bin/collapse-api

# --- runtime -----------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# curl is here only so the HEALTHCHECK below has something to run; a distroless
# base would be smaller but has no shell and no client to probe /health with.
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --user-group --no-create-home collapse \
    && mkdir -p /var/lib/collapse \
    && chown collapse:collapse /var/lib/collapse

COPY --from=builder /usr/local/bin/collapse-api /usr/local/bin/collapse-api

USER collapse
WORKDIR /var/lib/collapse
EXPOSE 8000

# No VOLUME declaration on purpose: it would make every plain "docker run" leave
# an anonymous volume behind. The compose file mounts a named one instead.

# The healthcheck assumes the default port. Override it too if you pass --port.
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8000/health || exit 1

# --host 0.0.0.0 lives in the ENTRYPOINT, not in a flag the operator has to
# remember: the server's own default is 127.0.0.1, which inside a container is
# the container's loopback, so publishing a port would silently reach nothing.
# Extra flags are appended, e.g. `docker run collapse-api:dev --max-upload-mb 50`
# (clap takes the last occurrence, so even --host can still be overridden).
ENTRYPOINT ["/usr/local/bin/collapse-api", "--host", "0.0.0.0", "--storage-dir", "/var/lib/collapse"]
