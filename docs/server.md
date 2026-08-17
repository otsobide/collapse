# Server

Collapse can compress on another machine. Two apps make that up:

- **`apps/server-backend`** (`collapse-server-backend`) — the HTTP API. It owns
  the compression engine and the job flow.
- **`apps/server-frontend`** (`collapse-server-frontend`) — a Vue web app for
  it, so a browser can compress without installing anything.
- **`apps/server-aio`** — both of the above in **one container**, which is
  usually the thing you actually want to deploy. It builds no new code: the
  same binary, the same bundle, the same nginx template.

Three clients talk to the backend: this web app, the CLI's `--server` flag and
the desktop app's destination picker. The two native ones share
[`collapse-remote`](architecture.md#collapse-remote--the-client-for-a-remote-server);
the web app speaks the same flow in JavaScript.

Neither app is a release artifact. You run them yourself, from this repository.

## Running the stack

```bash
make docker/up        # build and start both, then:
                      #   http://localhost:8080        the web app
                      #   http://localhost:8000/docs   the API's own docs
make docker/logs      # follow both containers
make docker/smoke     # build, start, drive a real compression, stop
make docker/down      # stop (the job volume survives)
make docker/clean     # remove containers, volume and images
```

Ports and limits come from the environment, so nothing has to be edited:

| Variable | Default | What it sets |
|---|---|---|
| `COLLAPSE_PORT` | `8000` | Host port for the API |
| `COLLAPSE_WEB_PORT` | `8080` | Host port for the web app |
| `COLLAPSE_MAX_UPLOAD_MB` | `500` | Largest accepted upload |

```bash
COLLAPSE_PORT=9000 COLLAPSE_WEB_PORT=9090 make docker/up
```

Both containers publish to **127.0.0.1 only**. That is deliberate: see
[Exposure](#exposure).

### One container instead of two

```bash
make docker/aio       # same two ports, same URLs, one container
```

`apps/server-aio` packs the API and the web app together: nginx serves the app
and proxies to a backend on the container's own loopback, and both ports are
published, so `:8080` is the web app and `:8000` is the API directly.

It is **packaging only**. The Dockerfile builds the same
`collapse-server-backend` binary and the same frontend bundle the split images
build, and copies the frontend's own `nginx.conf`. That matters: the reference
implementation grew a second, near-identical `main` for its all-in-one build,
and this deliberately does not.

Two consequences of running two processes under one roof:

- The entrypoint **traps `SIGTERM` and passes it to both**. Without that, PID 1
  ignores the signal and every `docker stop` waits out its grace period before
  killing the container. It stops instantly instead.
- If either process exits, the container exits, so a restart policy can bring
  the pair back together rather than leaving half a server answering.

It lives behind a compose profile because it claims the same ports as the pair
it replaces, so `make docker/up` and `make docker/aio` never collide. Check it
with `make server-aio/smoke`, which drives a real compression through **both**
published ports.

## Running without Docker

Useful while working on either half. The backend first, then the frontend,
which proxies to it:

```bash
cargo run -p collapse-server-backend -- --port 8000    # or make server-backend/run
make server-frontend/dev                               # Vite on port 5174
```

The backend's flags:

| Flag | Default | Notes |
|---|---|---|
| `--host` | `127.0.0.1` | The container image pins `0.0.0.0`, since a container's loopback reaches nothing from outside |
| `--port` | `8000` | |
| `--max-upload-mb` | `500` | Beyond it, uploads get a 413 |
| `--storage-dir` | a temporary directory | Removed when the process exits; give a path to keep jobs across restarts |

Point the dev server at a backend elsewhere with `COLLAPSE_BACKEND`, the same
variable the container image uses:

```bash
COLLAPSE_BACKEND=http://192.168.1.10:8000 make server-frontend/dev
```

## Why there is a proxy

The browser never makes a cross-origin request. nginx serves the built app and
proxies `/health`, `/compress`, `/jobs`, `/openapi.json` and `/docs` to the
backend; Vite proxies the same list in development.

That is not incidental. **The backend ships no CORS layer on purpose** (the
reference implementation shipped `CorsLayer::very_permissive()` in production,
which this project treats as a defect not to repeat). Same-origin requests need
none. If you ever see a CORS error here, the proxy is misconfigured; adding CORS
to the backend is the wrong fix.

Two settings in `nginx.conf` are load-bearing:

- `client_max_body_size` — nginx defaults to 1 MiB, which would turn every real
  upload into a 413 long before the backend's own cap applied.
- `proxy_read_timeout` / `proxy_send_timeout` — a job can run for minutes, and
  the default would cut it off mid-compression.

## What the web app does

The same shape as the desktop app: drop or choose a file, pick a format and a
level, watch it happen, save the archive. Two differences worth knowing:

- **Folders are packed in the browser.** HTTP carries no directory, so a
  `webkitdirectory` selection is written into a tar (`src/tar.js`) and sent with
  `envelope=tar`, which the backend unwraps. Empty folders do not survive,
  because a browser does not report them.
- **There is no extraction.** The backend compresses only, so there is nothing
  to call. Extracting would need a new endpoint and an answer to what the result
  should be when the output is a tree and the client is a browser.

## Exposure

The API has **no authentication and no rate limiting**, and speaks plain HTTP.
Everything below follows from that:

- Publishing the **web port also publishes the API**, because nginx proxies it.
  Exposing `8080` to a network exposes `/compress` and `/jobs` to it too.
- Uploaded content and downloaded archives travel **in the clear**. Terminate
  TLS in front of the stack if that matters.
- Jobs consume CPU, memory and disk, and the compose file sets a memory limit
  for that reason. Staged files are removed when a client deletes its job (all
  three clients do) or when the container goes away; abandoned jobs accumulate
  until then.

The full picture, including what the server does defend against when it unpacks
a tar someone sent it, is in [threat_model.md](threat_model.md#the-api-server).

## Testing

```bash
make server-backend/test     # 78 Rust tests, including a hostile-tar suite
make server-frontend/test    # 40 Vitest cases
make server-aio/smoke        # the packaged container, both ports (needs Docker)
```

The backend's suite drives the whole app in-process with no sockets; the
frontend's stubs `fetch`, so neither needs the other running. CI runs both, plus
a build of each. The Docker packaging is **not** in CI: `make docker/smoke` is
the way to check it, and it is worth running after touching a Dockerfile, the
compose file or the workspace layout.
