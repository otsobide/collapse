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

# Both processes are gone, but their last bytes may not be. A closed socket is
# still the kernel's to drain, and the container's network namespace dies with
# PID 1: exit now and whatever is queued is discarded, which a client reads as
# a download that ends short of its Content-Length. Measured on a 40 MB archive
# over a rate-limited connection, that was the last 4 MB.
#
# So wait for the kernel's own send queues to empty, bounded, and only for as
# long as something is actually pending.
queued_bytes() {
    # Column 5 of /proc/net/tcp is tx_queue:rx_queue, in hex. FNR, not NR: the
    # header has to be skipped in each file, and counting it as a socket makes
    # this never reach zero, turning the bounded wait into a flat delay on
    # every stop.
    local total=0 hex
    for hex in $(awk 'FNR > 1 { split($5, q, ":"); print q[1] }' /proc/net/tcp /proc/net/tcp6 2>/dev/null); do
        total=$((total + 0x$hex))
    done
    echo "$total"
}

# Empty once is not empty: the last bytes leave the namespace before they reach
# the client, through the port forwarding on the other side of it, so the exit
# waits for a short quiet spell rather than the first zero reading.
quiet=0
for _ in $(seq 1 100); do
    if [ "$(queued_bytes)" -eq 0 ]; then
        quiet=$((quiet + 1))
        [ "$quiet" -ge 5 ] && break
    else
        quiet=0
    fi
    sleep 0.1
done

exit "$status"
