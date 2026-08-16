#!/usr/bin/env bash
# Smoke test the containerised API through a published port: build the image,
# start a throwaway container, then drive the real job flow from the host
# (upload, poll, download, delete) and check the archive actually round-trips.
#
# It fails loudly on the first problem, so it is a real check rather than a
# "container started" formality.
#
# Usage: apps/server-backend/smoke.sh [port] [image]

set -euo pipefail

PORT="${1:-8000}"
IMAGE="${2:-collapse-server-backend:dev}"
NAME="collapse-server-backend-smoke-$$"
BASE="http://127.0.0.1:${PORT}"
WORK="$(mktemp -d)"

cleanup() {
  docker rm -f "$NAME" > /dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

step() { printf '\n\033[36m==> %s\033[0m\n' "$1"; }
fail() { printf '\033[31mFAIL: %s\033[0m\n' "$1" >&2; exit 1; }

step "Building $IMAGE"
docker build -f apps/server-backend/Dockerfile -t "$IMAGE" .

step "Starting a container on port $PORT"
docker run -d --rm --name "$NAME" -p "127.0.0.1:${PORT}:8000" "$IMAGE" > /dev/null

step "Waiting for the API to answer through the published port"
for _ in $(seq 1 60); do
  if curl -fsS "${BASE}/health" > /dev/null 2>&1; then break; fi
  sleep 0.5
done
curl -fsS "${BASE}/health" > /dev/null 2>&1 \
  || fail "the API never answered on ${BASE}/health (is the server bound to 0.0.0.0 inside the container?)"
echo "health: $(curl -fsS "${BASE}/health")"

step "Checking the self-documenting endpoints"
curl -fsS "${BASE}/openapi.json" | python3 -c \
  'import json,sys; s=json.load(sys.stdin); print("openapi", s["openapi"], "version", s["info"]["version"], "paths", len(s["paths"]))' \
  || fail "/openapi.json did not serve a valid document"
# Written to a file rather than piped into grep: grep -q closes the pipe on the
# first match, curl dies of SIGPIPE and pipefail would report a false failure.
curl -fsS "${BASE}/docs" -o "${WORK}/docs.html"
grep -q "<!doctype html>" "${WORK}/docs.html" || fail "/docs did not serve the page"
grep -qE "https?://" "${WORK}/docs.html" && fail "the served docs page references an external host"
echo "docs page: $(wc -c < "${WORK}/docs.html" | tr -d ' ') bytes, no external references"

step "Running a real compression through the API"
python3 -c "open('${WORK}/sample.txt','w').write('collapse in a container\n'*500)"
JOB_JSON="$(curl -fsS -X POST --data-binary "@${WORK}/sample.txt" \
  "${BASE}/compress?name=sample.txt&algorithm=zip&level=3")"
JOB_ID="$(printf '%s' "$JOB_JSON" | python3 -c 'import json,sys; print(json.load(sys.stdin)["job_id"])')"
echo "queued job $JOB_ID"

for _ in $(seq 1 120); do
  STATUS="$(curl -fsS "${BASE}/jobs/${JOB_ID}" | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])')"
  case "$STATUS" in
    completed) break ;;
    failed) fail "the server reported a failed job" ;;
  esac
  sleep 0.5
done
[ "$STATUS" = "completed" ] || fail "the job never completed (last status: $STATUS)"
echo "status: $STATUS"

curl -fsS -o "${WORK}/out.zip" "${BASE}/jobs/${JOB_ID}/download"
python3 - "$WORK" <<'PY' || exit 1
import sys, zipfile, pathlib
work = pathlib.Path(sys.argv[1])
with zipfile.ZipFile(work / "out.zip") as archive:
    names = archive.namelist()
    if names != ["sample.txt"]:
        raise SystemExit(f"FAIL: unexpected archive contents: {names}")
    if archive.read("sample.txt") != (work / "sample.txt").read_bytes():
        raise SystemExit("FAIL: the extracted file does not match the uploaded one")
print(f"archive verified: {names[0]}, {(work / 'out.zip').stat().st_size} bytes")
PY

step "Deleting the job"
curl -fsS -X DELETE "${BASE}/jobs/${JOB_ID}"
echo
# No -f here: a 404 is exactly what this asserts, and -f would make curl exit
# non-zero before the status code could be inspected.
GONE="$(curl -sS -o /dev/null -w '%{http_code}' "${BASE}/jobs/${JOB_ID}")"
[ "$GONE" = "404" ] || fail "the job still exists after being deleted (got HTTP $GONE)"

printf '\n\033[32mSmoke test passed: the containerised API works through port %s\033[0m\n' "$PORT"
