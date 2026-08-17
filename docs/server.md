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
| `COLLAPSE_PORT` | `8000` | Host port for the API (compose only) |
| `COLLAPSE_WEB_PORT` | `8080` | Host port for the web app (compose only) |
| `COLLAPSE_BACKEND` | `127.0.0.1:8000` | Where nginx proxies the API to. Only worth changing in the split setup, where it is `backend:8000`. |

The API binary's own flags, for a deployment without containers or for
overriding what the image pins:

| Flag | Default | Notes |
|---|---|---|
| `--host` | `127.0.0.1` | Both images pin `0.0.0.0`, since a container's loopback reaches nothing from outside |
| `--port` | `8000` | |
| `--max-upload-mb` | `500` | |
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

Two settings in [`nginx.conf`](../apps/server-frontend/nginx.conf) are
load-bearing: `client_max_body_size`, because nginx defaults to 1 MiB and would
turn every real upload into a 413 long before the backend's own cap applied,
and `proxy_read_timeout` / `proxy_send_timeout`, because a job can run for
minutes and the defaults would cut it off mid-compression.

## Operating it

### Health

Both images carry a `HEALTHCHECK`. The all-in-one probes **both** ports, since
a container serving only one of them is not useful. From outside:

```bash
curl -fsS http://localhost:8080/health     # through the proxy
docker inspect --format '{{.State.Health.Status}}' collapse
```

### Logs

Modest today: the API prints one line at startup and nginx logs its accesses.
Individual jobs are not logged, so a failed compression is visible in the job's
`error_message` and nowhere else.

```bash
docker logs -f collapse
```

### What lives in the volume

`/var/lib/collapse/<job_id>/` per job, holding the upload (`input/`), the
unpacked tree for a tar envelope (`tree/`) and the produced archive. Deleting
the job removes the whole directory.

**Nothing in it is worth backing up.** It is staging for in-flight work, not
data the server owns. Sizing it, worst case per job:

- a plain file: the upload plus the archive, so about twice the upload
- a tar envelope: the upload, the unpacked tree and the archive, about three
  times the upload

With the 500 MB default cap that is up to 1.5 GB of disk for a single job, and
concurrent uploads add up. Give the volume room, or lower the cap.

### Upgrades

```bash
docker compose build --pull && docker compose up -d
```

In-flight jobs do not survive: the registry is in memory, and the server does
not currently shut down gracefully. Upgrade when nothing is running.

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

- **Abandoned jobs are never cleaned up.** Staged files are removed when a
  client deletes its job, and all three clients do, but a client that stops
  half way leaves its directory behind. Nothing sweeps them, and a restart
  makes it permanent: the registry is in memory, so after a restart the API
  answers 404 for those jobs while their files stay on the volume. Recreate the
  volume periodically, or run without one and let a container restart wipe the
  staging area.
- **No graceful shutdown.** `docker stop` cuts in-flight uploads, downloads and
  compressions immediately.
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
make server-backend/test     # 78 Rust tests, including a hostile-tar suite
make server-frontend/test    # 40 Vitest cases
make server-aio/smoke        # the packaged container, both ports (needs Docker)
make docker/smoke            # the split stack through its published port
```

The backend's suite drives the whole app in-process with no sockets; the
frontend's stubs `fetch`, so neither needs the other running. CI runs both,
plus a build of each. The Docker packaging is **not** in CI: the two smoke
targets are the way to check it, and they are worth running after touching a
Dockerfile, the compose file or the workspace layout.
