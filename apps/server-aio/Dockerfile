# All-in-one Collapse server: the API and its web app in one container.
#
# This is packaging, not a third application. It builds the very same
# collapse-server-backend binary and the very same frontend bundle the split
# images use, and reuses the frontend's nginx config. Nothing here duplicates
# the server's main, which is exactly the trap the reference implementation
# fell into with its own all-in-one build.
#
# Build from the REPOSITORY ROOT:
#   docker build -f apps/server-aio/Dockerfile -t collapse-server-aio:dev .
#
# Ports: 8080 the web app (which also proxies the API), 8000 the API direct.

# --- build the API -----------------------------------------------------------
FROM rust:1.90-slim-bookworm AS backend

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY apps/core apps/core
COPY apps/remote apps/remote
COPY apps/cli apps/cli
COPY apps/server-backend apps/server-backend

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    cargo build -p collapse-server-backend --release --locked \
    && cp /app/target/release/collapse-server-backend /usr/local/bin/collapse-server-backend

# --- build the web app -------------------------------------------------------
FROM node:22-bookworm-slim AS frontend

WORKDIR /app
COPY apps/server-frontend/package.json apps/server-frontend/package-lock.json ./
RUN npm ci
COPY apps/server-frontend/ ./
RUN npm run build

# --- runtime -----------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# nginx serves the app and proxies the API; curl is for the healthcheck;
# gettext-base provides envsubst for the config template.
RUN apt-get update \
    && apt-get install -y --no-install-recommends nginx curl gettext-base \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --user-group --no-create-home collapse \
    && mkdir -p /var/lib/collapse \
    && chown collapse:collapse /var/lib/collapse

COPY --from=backend /usr/local/bin/collapse-server-backend /usr/local/bin/collapse-server-backend
COPY --from=frontend /app/dist /usr/share/nginx/html
# The same template the standalone frontend image uses: one source of truth.
COPY apps/server-frontend/nginx.conf /etc/nginx/templates/collapse.conf.template
COPY apps/server-aio/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

# Inside this container the web app reaches the API over loopback.
ENV COLLAPSE_BACKEND=127.0.0.1:8000
ENV COLLAPSE_MAX_UPLOAD_MB=500
# How long a finished job survives without being downloaded again. 0 keeps
# every job until a client deletes it.
ENV COLLAPSE_JOB_TTL_MINUTES=60
# On a stop, how long transfers already in flight get to finish. Raise the
# container's own stop grace period alongside it, or Docker's SIGKILL arrives
# first and none of it applies.
ENV COLLAPSE_SHUTDOWN_GRACE_SECONDS=10

EXPOSE 8000 8080
VOLUME ["/var/lib/collapse"]

# Both ports have to answer for the container to be useful, so both are probed.
HEALTHCHECK --interval=10s --timeout=5s --start-period=8s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8000/health > /dev/null \
     && curl -fsS -o /dev/null http://127.0.0.1:8080/ || exit 1

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
