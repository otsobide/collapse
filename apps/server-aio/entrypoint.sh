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
    --max-upload-mb "${COLLAPSE_MAX_UPLOAD_MB:-500}" \
    --job-ttl-minutes "${COLLAPSE_JOB_TTL_MINUTES:-60}" \
    --shutdown-grace-seconds "${COLLAPSE_SHUTDOWN_GRACE_SECONDS:-10}" "$@" &
api=$!

nginx -g 'daemon off;' &
web=$!

# Pass a stop on to both children. Without a handler installed, PID 1 ignores
# SIGTERM outright, so `docker stop` would wait out its grace period and then
# kill the container instead of shutting it down.
#
# The two are told differently on purpose. nginx reads SIGTERM as "fast
# shutdown" and drops the connections it is proxying, which would cut short the
# very downloads the API is staying alive to finish; SIGQUIT is its graceful
# one. The API drains on SIGTERM and exits on its own deadline.
shutdown() {
    kill -TERM "$api" 2>/dev/null || true
    kill -QUIT "$web" 2>/dev/null || true
}
trap shutdown TERM INT

# Whichever goes first decides the container's fate: a child that dies on its
# own, or the stop that interrupts this wait.
#
# The `||` is load-bearing, not defensive. A trapped signal makes `wait` return
# 143, and under `set -e` that counts as a failed command, so the script would
# exit right here and take both children with it, mid-response. That is exactly
# the truncated download this shutdown handling exists to prevent.
status=0
wait -n "$api" "$web" || status=$?

shutdown

# Now let them leave. The API drains its in-flight requests and exits on its own
# deadline (--shutdown-grace-seconds); nginx finishes what it is proxying. PID 1
# staying alive until then is what keeps the container from tearing them down,
# so the container's stop grace period has to be the longer of the two.
wait "$api" 2>/dev/null || true
wait "$web" 2>/dev/null || true

exit "$status"
