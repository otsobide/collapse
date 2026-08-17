#!/bin/bash
# Run the API and nginx side by side, and treat them as one unit: if either
# stops, the container stops, so a restart policy can do its job instead of
# leaving half a server answering.
set -euo pipefail

# Only COLLAPSE_BACKEND is substituted; nginx's own $uri, $host and friends
# must survive the template untouched.
envsubst '${COLLAPSE_BACKEND}' \
    < /etc/nginx/templates/collapse.conf.template \
    > /etc/nginx/sites-enabled/default

# The API listens on every interface because port 8000 is published too, not
# only reached through the proxy.
collapse-server-backend \
    --host 0.0.0.0 \
    --port 8000 \
    --storage-dir /var/lib/collapse \
    --max-upload-mb "${COLLAPSE_MAX_UPLOAD_MB:-500}" "$@" &
api=$!

nginx -g 'daemon off;' &
web=$!

# Pass a stop on to both children. Without a handler installed, PID 1 ignores
# SIGTERM outright, so `docker stop` would wait out its grace period and then
# kill the container instead of shutting it down.
shutdown() {
    kill -TERM "$api" "$web" 2>/dev/null || true
}
trap shutdown TERM INT

# Whichever exits first decides the container's fate.
wait -n "$api" "$web"
status=$?
shutdown
wait || true
exit "$status"
