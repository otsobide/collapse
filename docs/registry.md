# The registry

The server's only mutable state, and the thing an operator ends up looking at
when something is odd. This document is about what it holds, what survives
what, and how to read or repair it by hand.

For running the server, see [server.md](server.md); this is the layer beneath
that.

## Two directories, on purpose

```
/var/lib/collapse/            the storage directory (--storage-dir)
├── registry/
│   └── jobs.db               SQLite: who exists, in what state (+ -wal, -shm)
└── jobs/
    └── <job_id>/             one directory per job
        ├── input/upload      the bytes as they arrived
        ├── tree/             a tar envelope, unpacked (directories only)
        └── archive.<ext>     what the client came for
```

The halves are split because they behave nothing alike: a few kilobytes
written constantly against gigabytes written once and read once. You can mount
one volume over the parent, or one over each, and put the database on a fast
disk and the archives on a big cheap one.

It also means **everything under `jobs/` is a job**. Nothing has to tell the
database apart from the work, because it is not there.

Every path here is built from values the server chose: a job id it generated,
and fixed names. Nothing a client sends ever becomes a path component.

## What survives what

| What happens | The registry | The staged files |
|---|---|---|
| The server is restarted | Kept | Kept |
| A job was **finished** when it stopped | Kept, downloadable, deletable | Kept |
| A job was **running** when it stopped | Kept, marked `failed` with a reason | Its partial output is left behind until the job is collected |
| Files exist that no row claims | — | Deleted at startup |
| A row exists whose files are gone | Dropped at startup | — |
| A finished job nobody downloads again | Deleted after `--job-ttl-minutes` | Same |
| The container is recreated, volume kept | Kept | Kept |
| No volume is mounted | Goes with the container | Goes with the container |
| `--storage-dir` is not given | Goes with the process (a temp dir) | Same |

The two automatic passes behind that table are the **startup reconciliation**
and the **reaper**; [server.md](server.md#jobs-are-collected) covers when they
run and how to configure them.

## Reading it yourself

The enums are stored as the strings they travel as, so the database is worth
opening with any SQLite client. Nothing here needs the server to be stopped:
it is in WAL mode, so a reader does not block the worker.

```bash
sqlite3 /var/lib/collapse/registry/jobs.db \
  "SELECT job_id, name, status, datetime(updated_at, 'unixepoch') AS touched
   FROM jobs ORDER BY updated_at DESC LIMIT 20;"
```

```bash
# What is actually taking up room, next to what the registry thinks exists.
du -sh /var/lib/collapse/jobs/* | sort -h | tail
sqlite3 /var/lib/collapse/registry/jobs.db "SELECT status, count(*) FROM jobs GROUP BY status;"
```

The columns are `job_id`, `name`, `archive_name`, `algorithm`, `level`,
`envelope`, `status`, `error_message`, `created_at`, `updated_at` (unix
seconds) and `server_version`, the build that wrote the row.

## Schema versions

`PRAGMA user_version` carries the schema version, and the server migrates
forward on open: an older database gains what it is missing and keeps its
rows.

**It refuses to open a database from a newer schema.** Downgrading is not
supported, and carrying on would mean reading columns this build does not
understand:

```
This registry was written by a newer Collapse (schema 3; this server
understands 2). Downgrading is not supported: run the newer version, or
start from an empty registry.
```

That is a startup failure, so the server exits rather than serving from a
state it cannot vouch for. The fix is to go back to the newer build, or to
move `jobs.db` aside and start clean, losing whatever jobs were in flight.

## Rows this build cannot read

A schema version cannot catch everything. Adding a compression format does not
add a column, so a database written by a newer build can be structurally
identical and still hold, say, `algorithm = 'zstd'`. That only surfaces when
the value is read back, and then it says so:

```json
{
  "detail": "This job was recorded by Collapse 0.9.0 and this server (0.5.1) cannot read it: unknown algorithm \"zstd\"."
}
```

Three things about that, all deliberate:

- **It is a 500.** The server does have a state it cannot interpret, which is
  its problem and not the caller's. A 4xx would tell a client to retry
  differently when nothing it does can help.
- **It affects that job and no other.** Nothing that deletes a job reads it
  first, so the reaper and the startup pass step over an unreadable row
  instead of stopping on it. Before that, one such row stopped disk being
  reclaimed for everything else, and a startup could refuse to boot with no
  way to remove the row through an API that was not running.
- **The row is still collectable.** It expires and is reaped like any other,
  or you can remove it by hand:

```bash
sqlite3 /var/lib/collapse/registry/jobs.db "DELETE FROM jobs WHERE job_id = '<id>';"
rm -rf /var/lib/collapse/jobs/<id>
```

Deleting the row without the directory is safe: the next startup sweeps files
no row claims.

## When something is wrong

**The server will not start.** Read the log: it says which of the two it could
not do, opening the registry or using the staging directory. A registry from a
newer schema is the interesting case (above); the rest is usually permissions
on a mounted volume, since the split images run as an unprivileged user.

**Disk is filling.** Compare `du -sh /var/lib/collapse/jobs/*` against the
registry. Directories the registry does not know about will go at the next
startup. Jobs that are known and finished go when their window passes; lower
`--job-ttl-minutes` if the window is too generous for the disk you have.

**A client says its job vanished.** Either it was deleted (by the client, or
by the reaper after its window) or the server was restarted while it was
compressing, in which case the job is still there and `failed`, with a message
saying so.

**Starting over.** The registry holds work in progress, not data the server
owns. Stopping the server and deleting both directories is always a valid
recovery; the cost is the jobs nobody has downloaded yet.
