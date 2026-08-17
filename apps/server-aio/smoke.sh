#!/usr/bin/env bash
# Smoke test the all-in-one image: both published ports have to work, the web
# app has to be served, and a real compression has to round-trip through each.
set -euo pipefail

IMAGE="${1:-collapse-server-aio:dev}"
API_PORT="${2:-8000}"
WEB_PORT="${3:-8080}"
NAME="collapse-aio-smoke-$$"
WORK="$(mktemp -d)"

cleanup() {
  docker rm -f "$NAME" > /dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

step() { printf '\n\033[36m==> %s\033[0m\n' "$1"; }
fail() { printf '\033[31mFAIL: %s\033[0m\n' "$1" >&2; exit 1; }

step "Building $IMAGE"
docker build -f apps/server-aio/Dockerfile -t "$IMAGE" .

step "Starting one container with both ports published"
docker run -d --rm --name "$NAME" \
  -p "127.0.0.1:${API_PORT}:8000" -p "127.0.0.1:${WEB_PORT}:8080" "$IMAGE" > /dev/null

for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:${API_PORT}/health" > /dev/null 2>&1 \
     && curl -fsS -o /dev/null "http://127.0.0.1:${WEB_PORT}/" 2>/dev/null; then break; fi
  sleep 0.5
done

step "Both ports answer"
curl -fsS "http://127.0.0.1:${API_PORT}/health" || fail "the API port never answered"
echo " <- :${API_PORT} (API direct)"
curl -fsS -o "${WORK}/index.html" "http://127.0.0.1:${WEB_PORT}/" || fail "the web port never answered"
grep -q "<div id=\"app\"" "${WORK}/index.html" || fail "the web port did not serve the app"
echo "web app: $(wc -c < "${WORK}/index.html" | tr -d ' ') bytes on :${WEB_PORT}"
curl -fsS "http://127.0.0.1:${WEB_PORT}/health" || fail "the web port does not proxy the API"
echo " <- :${WEB_PORT} (API through the proxy)"

# A compression through each port, proving both paths reach the same engine.
compress_through() {
  local port="$1" label="$2"
  python3 -c "open('${WORK}/${label}.txt','w').write('all in one\n'*200)"
  local job id status
  job="$(curl -fsS -X POST --data-binary "@${WORK}/${label}.txt" \
    "http://127.0.0.1:${port}/compress?name=${label}.txt&algorithm=zip")"
  id="$(printf '%s' "$job" | python3 -c 'import json,sys; print(json.load(sys.stdin)["job_id"])')"
  for _ in $(seq 1 60); do
    status="$(curl -fsS "http://127.0.0.1:${port}/jobs/${id}" \
      | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])')"
    [ "$status" = "completed" ] && break
    [ "$status" = "failed" ] && fail "the job failed via $label"
    sleep 0.5
  done
  [ "$status" = "completed" ] || fail "the job never completed via $label (last: $status)"
  curl -fsS -o "${WORK}/${label}.zip" "http://127.0.0.1:${port}/jobs/${id}/download"
  python3 - "$WORK" "$label" <<'PY'
import sys, zipfile, pathlib
work, label = pathlib.Path(sys.argv[1]), sys.argv[2]
with zipfile.ZipFile(work / f"{label}.zip") as archive:
    if archive.namelist() != [f"{label}.txt"]:
        raise SystemExit(f"FAIL: unexpected contents: {archive.namelist()}")
    if archive.read(f"{label}.txt") != (work / f"{label}.txt").read_bytes():
        raise SystemExit("FAIL: the archive does not match what was uploaded")
print(f"  verified via {label}: {(work / f'{label}.zip').stat().st_size} bytes")
PY
  curl -fsS -X DELETE "http://127.0.0.1:${port}/jobs/${id}" > /dev/null
}

step "Compressing through the API port"
compress_through "$API_PORT" "direct"

step "Compressing through the web port's proxy"
compress_through "$WEB_PORT" "proxied"

step "Both processes are alive in the one container"
# Read /proc rather than pulling in procps just for this. Note comm is
# truncated to 15 characters, hence the shortened name.
docker exec "$NAME" sh -c 'grep -lx nginx /proc/*/comm' > /dev/null \
  || fail "nginx is not running in the container"
docker exec "$NAME" sh -c 'grep -lx collapse-server /proc/*/comm' > /dev/null \
  || fail "the API is not running in the container"
echo "nginx and the API are both running"

printf '\n\033[32mAll-in-one smoke test passed on :%s and :%s\033[0m\n' "$API_PORT" "$WEB_PORT"
