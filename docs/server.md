# Server

Collapse can compress on another machine. This document is the operational
guide for that server: how to run it, how to configure it, what its API looks
like, and what it does not do yet.

Three apps make it up:

| App | What it is |
|---|---|
| [`apps/server-backend`](../apps/server-backend) (`collapse-server-backend`) | The HTTP API. It owns the compression engine and the job flow. |
| [`apps/server-frontend`](../apps/server-frontend) (`collapse-server-frontend`) | A Vue web app for it, so a browser can compress without installing anything. |
| [`apps/server-aio`](../apps/server-aio) | Both of the above in **one container**, which is usually the thing you actually want to deploy. It builds no new code: the same binary, the same bundle, the same nginx config. |

Three clients talk to the backend: this web app, the CLI's `--server` flag and
the desktop app's destination picker. The two native ones share
[`collapse-remote`](architecture.md#collapse-remote--the-client-for-a-remote-server);
the web app speaks the same flow in JavaScript.

## Quick start

There are no published container images yet, so the images are built from this
repository:

```bash
make docker/aio          # build and start the all-in-one container
```

That serves the web app on <http://localhost:8080> and the API on
<http://localhost:8000>, whose own interactive documentation is at
<http://localhost:8000/docs>.

Without the Makefile, the same thing by hand:

```bash
docker build -f apps/server-aio/Dockerfile -t collapse-server-aio:dev .   # from the repo root
docker run -d --name collapse \
  -p 127.0.0.1:8080:8080 \
  -v collapse-jobs:/var/lib/collapse \
  collapse-server-aio:dev
```

The build context is the **repository root** in every case: the backend depends
on `collapse-core` through a path dependency, so the whole workspace has to be
in the context.

### Split containers instead

Useful when the web app and the API belong on different hosts, or when you want
only the API:

```bash
make docker/up        # backend + frontend as two containers
make docker/logs      # follow both
make docker/down      # stop (the job volume survives)
make docker/clean     # remove containers, volume and images
```

## Configuration

The all-in-one image is configured through the environment; the API binary
takes flags. Everything has a working default.

| Variable | Default | What it sets |
|---|---|---|
| `COLLAPSE_MAX_UPLOAD_MB` | `500` | Largest accepted upload, in MiB. Beyond it, uploads get a 413. |
| `COLLAPSE_JOB_TTL_MINUTES` | `60` | How long a finished job survives without being downloaded again. `0` keeps every job until a client deletes it. See [Jobs are collected](#jobs-are-collected). |
| `COLLAPSE_PORT` | `8000` | Host port for the API (compose only) |
| `COLLAPSE_WEB_PORT` | `8080` | Host port for the web app (compose only) |
| `COLLAPSE_BACKEND` | `127.0.0.1:8000` | Where nginx proxies the API to. Only worth changing in the split setup, where it is `backend:8000`. |
| `COLLAPSE_SHUTDOWN_GRACE_SECONDS` | `10` | On a stop, how long transfers already in flight get to finish. Raise the container's `stop_grace_period` alongside it. See [Stopping it](#stopping-it). |
| `RUST_LOG` | `info` | Log level, or a per-target spec like `collapse_server_backend=debug,tower_http=warn`. See [Logs](#logs). |

The API binary's own flags, for a deployment without containers or for
overriding what the image pins:

| Flag | Default | Notes |
|---|---|---|
| `--host` | `127.0.0.1` | Both images pin `0.0.0.0`, since a container's loopback reaches nothing from outside |
| `--port` | `8000` | |
| `--max-upload-mb` | `500` | |
| `--job-ttl-minutes` | `60` | `0` disables the reaper |
| `--shutdown-grace-seconds` | `10` | On a stop, how long transfers already in flight get to finish |
| `--storage-dir` | a temporary directory | Removed when the process exits. Give a path to keep jobs across restarts. Both images pin `/var/lib/collapse`. |

Anything you append to `docker run` reaches the binary, and clap takes the last
occurrence of a flag, so the pinned values can still be overridden:

```bash
docker run collapse-server-aio:dev --max-upload-mb 50
```

Ports and paths inside the container are fixed: **8080** the web app, **8000**
the API, `/var/lib/collapse` the job staging directory.

## Deploying it in your own compose

The all-in-one image is a single service. This is a complete file to copy:

```yaml
name: collapse

services:
  collapse:
    image: collapse-server-aio:dev      # built from this repository
    container_name: collapse
    restart: unless-stopped

    environment:
      COLLAPSE_MAX_UPLOAD_MB: "500"
      # Finished jobs nobody downloads again are collected after this long, so
      # a client that walks away does not leave its files on the volume.
      COLLAPSE_JOB_TTL_MINUTES: "60"

    # Only the web port is published. It proxies the API, so :8080 alone is a
    # complete deployment; publish 8000 as well only if a CLI or desktop client
    # has to reach the API directly.
    ports:
      - "127.0.0.1:8080:8080"

    # Job staging. Uploads and archives live here, so keep them off the
    # container's writable layer. Nothing in it needs backing up: see
    # "What lives in the volume".
    volumes:
      - jobs:/var/lib/collapse

    # The zip and 7z backends buffer whole files in memory and nothing caps an
    # archive's expansion, so a container-level ceiling is what stands between
    # a large upload and the host's RAM. Size it well above the upload cap.
    mem_limit: 2g

    # A stop lets transfers already in flight finish. Docker's default here is
    # ten seconds, the same as the server's own deadline, which would SIGKILL
    # just as it was about to exit cleanly.
    stop_grace_period: 20s

volumes:
  jobs:
```

Then `docker compose up -d`. Notes on the choices above, worth keeping in your
own file:

- **Publish on `127.0.0.1` unless something in front terminates TLS.** The API
  has no authentication and no rate limiting; see [Exposure](#exposure).
- **Publishing the web port publishes the API too**, because nginx proxies it.
  There is no configuration in which `:8080` is safe and `:8000` is not.
- **`restart: unless-stopped` is the right policy**: if either process inside
  the container dies, the container exits on purpose so the pair is restarted
  together rather than left half-serving.

## Behind a reverse proxy

Two settings decide whether large uploads work, and both live in *your* proxy
as well as in the container's own nginx: a body-size limit big enough for the
uploads you allow, and read/send timeouts long enough for a job to finish. The
defaults of most proxies are too small on both counts.

Caddy:

```caddyfile
collapse.example.com {
    reverse_proxy 127.0.0.1:8080 {
        transport http {
            read_timeout 3600s
            write_timeout 3600s
        }
    }
    request_body {
        max_size 500MB
    }
}
```

nginx:

```nginx
server {
    listen 443 ssl;
    server_name collapse.example.com;

    # At least COLLAPSE_MAX_UPLOAD_MB, or your proxy rejects what the server
    # would have accepted.
    client_max_body_size 500m;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
        proxy_request_buffering off;
    }
}
```

TLS is worth terminating there: the server speaks plain HTTP and uploads travel
in the clear.

## The API

Compression is asynchronous. Uploading answers `202 Accepted` with a job while
a background worker compresses; the job is polled, the archive downloaded once
it completes, and the job deleted afterwards.

| Method and path | What it does |
|---|---|
| `GET /health` | Liveness probe: `{"status":"ok"}` |
| `POST /compress?name=&algorithm=&level=&envelope=` | Upload raw bytes, get a queued job (202) |
| `GET /jobs/{job_id}` | The job's current state |
| `GET /jobs/{job_id}/download` | The archive bytes, once completed |
| `DELETE /jobs/{job_id}` | Drop the job and its files |
| `GET /docs` | Interactive documentation, served by the binary itself |
| `GET /openapi.json` | OpenAPI 3.1 description of this server |

Query parameters of `POST /compress`:

| Parameter | Required | Default | Values |
|---|---|---|---|
| `name` | yes | | A bare file name. It becomes the name inside the archive, so no separators, no `..`. |
| `algorithm` | no | `zip` | `zip`, `7z`, `tar` (`tar` only bundles, it does not compress) |
| `level` | no | `3` | `1` to `5`. Out of range is a 400, never silently clamped. |
| `envelope` | no | `none` | `none` for a plain file, `tar` for a directory: see below. |

The body is the file's raw bytes. There is no multipart form.

### A file, end to end

```bash
# 1. Upload. The response is the job, and 202 means "queued", not "done".
curl -sS -X POST --data-binary @notes.txt \
  "http://localhost:8080/compress?name=notes.txt&algorithm=zip&level=3"
```

```json
{
  "job_id": "3f2a…",
  "name": "notes.txt",
  "archive_name": "notes.txt.zip",
  "algorithm": "zip",
  "level": 3,
  "envelope": "none",
  "status": "queued",
  "error_message": null
}
```

```bash
# 2. Poll until it leaves queued/compressing.
curl -sS "http://localhost:8080/jobs/$JOB_ID"
# {"status":"completed", …}

# 3. Download. The archive name comes back in Content-Disposition.
curl -sS -OJ "http://localhost:8080/jobs/$JOB_ID/download"

# 4. Delete, which removes the staged files. Nothing else does.
curl -sS -X DELETE "http://localhost:8080/jobs/$JOB_ID"
# {"job_id":"3f2a…","deleted":true}
```

`status` moves `queued` to `compressing` to `completed` or `failed`. On
`failed`, `error_message` says why.

### A directory

HTTP carries no directory, so a client packs one into a **tar** and says so
with `envelope=tar`. The server unpacks it and compresses the tree it holds:

```bash
tar -cf photos.tar photos          # one directory at the root of the tar
curl -sS -X POST --data-binary @photos.tar \
  "http://localhost:8080/compress?name=photos&envelope=tar&algorithm=7z&level=5"
```

`name` is the **directory's** name, not the tar's. The envelope is never
inferred from the file name: `photos.tar` may equally be a file you want
compressed as it is, and guessing would make that case impossible to ask for.

Tar is the envelope because it does not compress. A zip or 7z envelope would
let a small upload expand without bound on the server, which the upload cap
could not contain.

### Errors

Errors are JSON, always shaped `{"detail": "..."}`:

| Status | When |
|---|---|
| 400 | Bad `name`, unknown `algorithm` or `envelope`, `level` out of range, a tar envelope whose contents are not exactly one directory named `name` |
| 404 | No such job, or its archive is gone |
| 409 | Downloading or deleting a job that is still queued or compressing, or downloading one that failed |
| 413 | The upload is larger than `--max-upload-mb` |
| 500 | Something failed server-side while staging the upload |

## Clients

**The CLI** compresses remotely with `--server`, for files and directories
alike (a directory is packed into a tar envelope for you):

```bash
collapse compress notes.txt --server http://collapse.example.com
collapse compress ./photos -f 7z -l 5 --server http://collapse.example.com
```

The archive lands beside the source, exactly as a local run would. The safety
guards run before any network traffic, so a run that would overwrite your
source or an existing archive fails without uploading a byte.

**The desktop app** has a destination picker (`Where`) that defaults to `This
computer`; the gear in the header opens a panel to add, test and remove
servers. Added servers persist between launches. Extraction is always local:
the server does not extract.

**The web app** is the same shape as the desktop app: drop a file or choose a
folder, pick a format and a level, watch it happen, save the archive. Folders
are packed into a tar in the browser, so empty folders do not survive (a
browser does not report them).

## Why there is a proxy

The browser never makes a cross-origin request. nginx serves the built app and
proxies `/health`, `/compress`, `/jobs`, `/openapi.json` and `/docs` to the
backend; Vite proxies the same list in development.

That is not incidental. **The backend ships no CORS layer on purpose** (the
reference implementation shipped `CorsLayer::very_permissive()` in production,
which this project treats as a defect not to repeat). Same-origin requests need
none. If you ever see a CORS error here, the proxy is misconfigured; adding
CORS to the backend is the wrong fix.

Three settings in [`nginx.conf`](../apps/server-frontend/nginx.conf) are
load-bearing: `client_max_body_size`, because nginx defaults to 1 MiB and would
turn every real upload into a 413 long before the backend's own cap applied;
`proxy_read_timeout` / `proxy_send_timeout`, because a job can run for minutes
and the defaults would cut it off mid-compression; and `proxy_buffering off`,
because the default writes every download to a temp file of nginx's own before
feeding the client, so a 500 MB archive is written twice and the proxy needs
disk for a copy of what the backend just produced.

## Operating it

### Health

Both images carry a `HEALTHCHECK`. The all-in-one probes **both** ports, since
a container serving only one of them is not useful. From outside:

```bash
curl -fsS http://localhost:8080/health     # through the proxy
docker inspect --format '{{.State.Health.Status}}' collapse
```

### Logs

The API logs through [`tracing`](https://docs.rs/tracing), so every line
carries an RFC 3339 timestamp, a level and the module that emitted it, on
stdout:

```bash
docker logs -f collapse
```

```
2026-08-17T18:56:48.325814Z  INFO collapse_server_backend: collapse-server-backend listening version="0.5.1" addr=0.0.0.0:8000 max_upload_mb=500 storage_dir=/var/lib/collapse
2026-08-17T18:56:48.651681Z  INFO request{method=POST uri=/compress?name=notes.txt&algorithm=zip&level=5}: collapse_server_backend::routes: queued job=7b6015d0 name=notes.txt algorithm=zip level=5 envelope=none bytes=45000
2026-08-17T18:56:48.651818Z  INFO request{method=POST uri=/compress?name=notes.txt&algorithm=zip&level=5}: tower_http::trace::on_response: finished processing request latency=0 ms status=202
2026-08-17T18:56:48.651887Z  INFO collapse_server_backend::queue: compressing job=7b6015d0 name=notes.txt algorithm=zip level=5
2026-08-17T18:56:48.653465Z  INFO collapse_server_backend::queue: completed job=7b6015d0 bytes=233 elapsed_ms=1
2026-08-17T18:56:48.719158Z  INFO request{method=DELETE uri=/jobs/7b6015d0}: collapse_server_backend::routes: deleted job=7b6015d0 files_removed=true
```

What you get at the default level:

- **One line per HTTP request**, with its method, path, status and latency.
- **The job lifecycle**: `queued` (with the upload size), `compressing`,
  and then `completed` (with the archive size and how long it took) or
  `failed`. Every line carries `job=<id>`, so one job's story greps out of a
  busy log in one go.
- **Failures at the level they deserve**: a rejected upload or a hostile tar is
  a `WARN`, because it is the client's problem; a worker that dies mid-job is
  an `ERROR`, because it is ours.

`GET /health` is deliberately **not** logged: a container probes it every ten
seconds and those lines would bury everything worth reading. Its failures show
up in the container's health status instead.

Turn the volume up or down with `RUST_LOG`, the usual spelling:

```bash
RUST_LOG=debug docker compose up -d              # everything
RUST_LOG=collapse_server_backend=info,tower_http=warn docker compose up -d   # jobs, no requests
```

An unparseable `RUST_LOG` is ignored with a warning rather than taken as
"log nothing", so a typo cannot silence the server.

The web app's nginx logs its own accesses separately, in the same stream.

### What lives in the volume

`/var/lib/collapse/<job_id>/` per job, holding the upload (`input/`), the
unpacked tree for a tar envelope (`tree/`) and the produced archive. Deleting
the job removes the whole directory.

Beside those directories sits **`jobs.db`**, the SQLite registry (with its
`-wal` and `-shm` companions). It is what lets a job survive a restart, and the
reason the staging area must be a persistent volume if you care about that:
every *directory* in there is a job, and anything that is not claimed by a row
in the database is deleted at startup.

### Jobs are collected

Every client deletes its job after downloading, and all three of ours do. For
the ones that do not, because a browser tab closed or a script died half way,
the server sweeps: **a finished job nobody downloads again within
`COLLAPSE_JOB_TTL_MINUTES` (an hour by default) is deleted**, row and files
together, and says so:

```
INFO collapse_server_backend: reaped jobs nobody came back for jobs=3 ttl_minutes=60
```

Three rules make that safe to leave running unattended:

- **Downloading restarts the clock.** Polling does not, deliberately: a client
  that watches a finished job forever without ever fetching it has abandoned
  it in every sense that matters to disk.
- **Work in progress is never collected**, however old it looks. Those jobs
  belong to the worker, and deleting one under it would leave a compression
  writing into a directory that no longer exists.
- **`0` turns it off**, for a deployment that would rather keep every job until
  a client asks for it to go.

Together with the startup pass, that bounds the disk: what is on it is the jobs
of the last hour, plus whatever is running right now.

**Nothing in it is worth backing up.** It is staging for in-flight work, not
data the server owns. Sizing it, worst case per job:

- a plain file: the upload plus the archive, so about twice the upload
- a tar envelope: the upload, the unpacked tree and the archive, about three
  times the upload

With the 500 MB default cap that is up to 1.5 GB of disk for a single job, and
concurrent uploads add up. Give the volume room, or lower the cap.

### Stopping it

A stop is graceful: the server **stops accepting connections and finishes the
requests it already has**, then exits.

```
INFO collapse_server_backend: stopping: no new connections, finishing what is in flight grace_seconds=10
INFO collapse_server_backend: stopped
```

That matters most for downloads. Without it, restarting while someone was
fetching a 40 MB archive handed them a truncated file; now the transfer
completes and the server leaves afterwards. A clean exit is also what removes
the default temporary staging directory, which a killed process leaves behind.

Two limits worth knowing:

- **`--shutdown-grace-seconds` (10) is a deadline, not a promise.** A client
  that stops reading its download cannot hold the server open forever, so once
  the window passes the process exits anyway.
- **Docker's own grace period has to be at least as long.** It defaults to ten
  seconds and then sends SIGKILL, which would cut the drain short at exactly
  the wrong moment; the compose file gives both services `stop_grace_period:
  20s`. The web container also gets `stop_signal: SIGQUIT`, because nginx reads
  SIGTERM as "fast shutdown" and would drop the connections it is proxying.
- **A container that stops still costs the tail of a transfer.** The processes
  drain, but a container's network namespace dies with PID 1 and takes with it
  whatever the kernel had queued and not yet delivered, so a client downloading
  at the moment of the stop can end up short of its Content-Length. That is a
  detected failure, not a silent one: the archive's length is known, so clients
  see a broken transfer rather than a corrupt file. The CLI, for one, reports
  `response body closed before all bytes were read` and writes nothing.
  Downloading again is all it takes, since the job and its archive are still
  there.

**Compression in progress is not waited for.** A job that was running when the
server stopped comes back `failed`, with `error_message` saying the server
restarted, so a client is told rather than left polling forever. Waiting for it
would mean holding a stop open for as long as an archive takes, which no
container runtime will allow anyway.

### Upgrades

```bash
docker compose build --pull && docker compose up -d
```

Finished jobs survive it: the registry is on disk, so a client can still poll,
download and delete across the restart. In-flight transfers survive it too, as
long as they fit in the grace period. Jobs mid-compression do not, so it is
still worth upgrading when nothing is running.

The reconciliation that does this reports itself when it finds anything:

```
INFO collapse_server_backend: reconciled the registry with the staging directory interrupted=0 without_files=0 orphaned=1
```

## Exposure

The API has **no authentication and no rate limiting**, and speaks plain HTTP.
Everything below follows from that:

- Publishing the **web port also publishes the API**, because nginx proxies it.
  Exposing `8080` to a network exposes `/compress` and `/jobs` to it too.
- Anyone who can reach it can spend your CPU, RAM and disk. Put it behind
  something that authenticates, or keep it on a trusted network.
- Uploaded content and downloaded archives travel **in the clear**. Terminate
  TLS in front of the stack if that matters.
- Job IDs are random UUIDs, so they cannot be guessed from one another, but
  anyone holding an ID can download or delete that job.

The full picture, including what the server does defend against when it unpacks
a tar someone sent it, is in [threat_model.md](threat_model.md#the-api-server).

## Known limitations

Worth knowing before deploying this unattended:

- **A job can outlive its client's patience.** Nothing is left behind any more
  (see [Jobs are collected](#jobs-are-collected)), but the flip side is that a
  client which comes back for an archive more than an hour after downloading it
  finds a 404. Raise `COLLAPSE_JOB_TTL_MINUTES` if your clients work that way.
- **A stop still costs whatever is compressing.** Requests in flight are
  finished (see [Stopping it](#stopping-it)), but the worker is not waited for,
  so a job mid-compression comes back `failed` and has to be uploaded again.
- **Uploads and downloads are buffered whole in memory** by the backend, on top
  of what the compression itself buffers. Keep `mem_limit` well above
  `COLLAPSE_MAX_UPLOAD_MB` and expect concurrency to multiply it.
- **The queue is unbounded.** Jobs are compressed one at a time, which protects
  the CPU, but nothing limits how many can be queued or how much disk they take
  together.
- **The all-in-one container runs as root**, unlike the split backend image,
  which drops to an unprivileged user.
- **No extraction over HTTP.** The server compresses only, so the web app has
  no extract mode. Adding one needs an endpoint and an answer to what the
  result should be when the output is a tree and the client is a browser.

## Running without Docker

Useful while working on either half. The backend first, then the frontend,
which proxies to it:

```bash
cargo run -p collapse-server-backend -- --port 8000    # or make server-backend/run
make server-frontend/dev                               # Vite on port 5174
```

Point the dev server at a backend elsewhere with `COLLAPSE_BACKEND`, the same
variable the container image uses:

```bash
COLLAPSE_BACKEND=http://192.168.1.10:8000 make server-frontend/dev
```

## Testing

```bash
make server-backend/test     # 129 Rust tests: unit, a hostile-tar suite, and
                             # an end-to-end suite that runs the real binary
make server-frontend/test    # 42 Vitest cases
make server-aio/smoke        # the packaged container, both ports (needs Docker)
make docker/smoke            # the split stack through its published port
```

Most of the backend's suite drives the app in-process with no sockets, which is
fast and precise. `tests/e2e.rs` is the other half: it **launches the real
binary** on a port the operating system picks, drives every endpoint over a
real socket, and covers the things no in-process test can reach, starting with
a job outliving the process that made it. The frontend's suite stubs `fetch`,
so neither side needs the other running. CI runs both, plus a build of each. The Docker packaging is **not** in CI: the two smoke
targets are the way to check it, and they are worth running after touching a
Dockerfile, the compose file or the workspace layout.
