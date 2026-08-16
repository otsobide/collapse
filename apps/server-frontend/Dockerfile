# Container image for the Collapse web frontend.
#
# Build from the REPOSITORY ROOT so the context matches the backend's:
#   docker build -f apps/server-frontend/Dockerfile -t collapse-server-frontend:dev .
#
# Bump both image tags together when moving to newer bases.

# --- build -------------------------------------------------------------------
FROM node:22-bookworm-slim AS builder

WORKDIR /app

# Dependencies first: editing a component must not reinstall node_modules.
COPY apps/server-frontend/package.json apps/server-frontend/package-lock.json ./
RUN npm ci

COPY apps/server-frontend/ ./
RUN npm run build

# --- runtime -----------------------------------------------------------------
FROM nginx:1.27-alpine AS runtime

# The upstream is templated so one image works against any backend address.
ENV COLLAPSE_BACKEND=backend:8000
COPY apps/server-frontend/nginx.conf /etc/nginx/templates/default.conf.template
COPY --from=builder /app/dist /usr/share/nginx/html

EXPOSE 8080

# nginx:alpine already substitutes /etc/nginx/templates/*.template at startup
# and runs as root only to bind the port; the workers drop to nginx.
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
    CMD wget -qO- http://127.0.0.1:8080/ > /dev/null || exit 1
