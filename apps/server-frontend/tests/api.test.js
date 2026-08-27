import { describe, it, expect, vi } from 'vitest'
import {
  compress,
  health,
  progressOf,
  nextDelay,
  FIRST_POLL_DELAY,
  MAX_POLL_DELAY,
} from '../src/api.js'

/** A fetch stub driven by a list of canned responses, in order. */
function fetcherFrom(responses) {
  const calls = []
  const fetcher = vi.fn(async (url, options = {}) => {
    calls.push({ url, method: options.method || 'GET' })
    const next = responses.shift()
    if (!next) throw new Error(`unexpected request: ${url}`)
    return next
  })
  return { fetcher, calls }
}

const json = (body, ok = true, status = 200) => ({
  ok,
  status,
  json: async () => body,
  blob: async () => new Blob(['archive']),
})

describe('progressOf', () => {
  it('waits while the job is in progress', () => {
    expect(progressOf({ status: 'queued' })).toBe('waiting')
    expect(progressOf({ status: 'compressing' })).toBe('waiting')
  })

  it('is ready once completed', () => {
    expect(progressOf({ status: 'completed' })).toBe('ready')
  })

  it('surfaces the server message when the job failed', () => {
    expect(() => progressOf({ status: 'failed', error_message: 'disk full' })).toThrow(/disk full/)
    expect(() => progressOf({ status: 'failed' })).toThrow(/compression failed/)
  })

  /// Anything else means the server is not speaking this protocol; treating it
  /// as in-progress would poll forever, which is the bug the Rust client had.
  it('refuses to wait on a status it does not know', () => {
    expect(() => progressOf({ status: 'cancelled' })).toThrow(/unexpected job status/)
    expect(() => progressOf({})).toThrow(/no status/)
  })
})

describe('health', () => {
  it('accepts a Collapse server', async () => {
    const { fetcher } = fetcherFrom([json({ status: 'ok' })])
    await expect(health(fetcher)).resolves.toBe(true)
  })

  it('rejects something that answers but is not Collapse', async () => {
    const { fetcher } = fetcherFrom([json({ status: 'fine' })])
    await expect(health(fetcher)).rejects.toThrow(/not a Collapse server/)
  })
})

describe('compress', () => {
  it('runs the whole job flow and cleans up after itself', async () => {
    const { fetcher, calls } = fetcherFrom([
      json({ job_id: 'j1', status: 'queued', archive_name: 'notes.txt.zip' }, true, 202),
      json({ status: 'completed' }),
      json({}), // the download
      json({ deleted: true }),
    ])
    const seen = []

    const out = await compress(
      { body: new Blob(['x']), name: 'notes.txt', algorithm: 'zip', level: 3 },
      { fetcher, onStatus: (s) => seen.push(s) },
    )

    expect(out.name).toBe('notes.txt.zip')
    expect(calls.map((c) => `${c.method} ${c.url.split('?')[0]}`)).toEqual([
      'POST /compress',
      'GET /jobs/j1',
      'GET /jobs/j1/download',
      'DELETE /jobs/j1',
    ])
    // The UI shows the flow, so every state has to be reported.
    expect(seen).toEqual(['uploading', 'completed', 'downloading', 'done'])
  })

  it('sends the parameters the backend expects', async () => {
    const { fetcher, calls } = fetcherFrom([
      json({ job_id: 'j1', archive_name: 'photos.7z' }, true, 202),
      json({ status: 'completed' }),
      json({}),
      json({ deleted: true }),
    ])

    await compress(
      { body: new Blob(['x']), name: 'photos', algorithm: '7z', level: 5, envelope: 'tar' },
      { fetcher },
    )

    const query = new URL(`http://x${calls[0].url}`).searchParams
    expect(Object.fromEntries(query)).toEqual({
      name: 'photos',
      algorithm: '7z',
      level: '5',
      envelope: 'tar',
    })
  })

  /// A download that breaks half way is the one failure the status code cannot
  /// report: the 200 was sent with the headers, long before anything went
  /// wrong. The browser catches it when it reads the body, and this pins that
  /// the app never treats a half-delivered archive as a finished one.
  it('rejects a download that breaks half way instead of saving a partial archive', async () => {
    const { fetcher } = fetcherFrom([
      json({ job_id: 'j1', archive_name: 'notes.txt.zip' }, true, 202),
      json({ status: 'completed' }),
      {
        ok: true,
        status: 200,
        // What fetch does when the connection dies before Content-Length is
        // satisfied: the response is fine, reading its body is not.
        blob: async () => {
          throw new TypeError('network error')
        },
      },
    ])
    const seen = []

    await expect(
      compress(
        { body: new Blob(['x']), name: 'notes.txt', algorithm: 'zip', level: 3 },
        { fetcher, onStatus: (s) => seen.push(s) },
      ),
    ).rejects.toThrow()

    expect(seen).not.toContain('done')
    expect(seen.at(-1)).toBe('downloading')
  })

  /// And it leaves the job where it is, which is what makes retrying cheap:
  /// the archive is still on the server, so nothing has to be uploaded or
  /// compressed again.
  it('leaves the job on the server when the download breaks', async () => {
    const { fetcher, calls } = fetcherFrom([
      json({ job_id: 'j1', archive_name: 'notes.txt.zip' }, true, 202),
      json({ status: 'completed' }),
      {
        ok: true,
        status: 200,
        blob: async () => {
          throw new TypeError('network error')
        },
      },
    ])

    await expect(
      compress({ body: new Blob(['x']), name: 'notes.txt', algorithm: 'zip', level: 3 }, { fetcher }),
    ).rejects.toThrow()

    expect(calls.map((c) => c.method)).not.toContain('DELETE')
  })

  it('reports the server reason when the upload is rejected', async () => {
    const { fetcher } = fetcherFrom([json({ detail: 'Invalid file name.' }, false, 400)])

    await expect(
      compress({ body: new Blob(['x']), name: '..', algorithm: 'zip', level: 3 }, { fetcher }),
    ).rejects.toThrow(/Invalid file name/)
  })

  it('stops when the job fails on the server', async () => {
    const { fetcher } = fetcherFrom([
      json({ job_id: 'j1' }, true, 202),
      json({ status: 'failed', error_message: 'not a tar' }),
    ])

    await expect(
      compress({ body: new Blob(['x']), name: 'p', algorithm: 'zip', level: 3, envelope: 'tar' }, { fetcher }),
    ).rejects.toThrow(/not a tar/)
  })

  it('refuses a 202 with no job id rather than polling nothing', async () => {
    const { fetcher } = fetcherFrom([json({ status: 'queued' }, true, 202)])

    await expect(
      compress({ body: new Blob(['x']), name: 'a.txt', algorithm: 'zip', level: 3 }, { fetcher }),
    ).rejects.toThrow(/no job_id/)
  })
})

describe('the poll schedule', () => {
  /**
   * Record every delay the loop asks for and fire it at once, so a schedule
   * spanning seconds of nominal waiting is tested in no time at all and with
   * no wall clock in the assertions.
   */
  function captureDelays() {
    const waits = []
    const real = globalThis.setTimeout
    vi.spyOn(globalThis, 'setTimeout').mockImplementation((fn, ms) => {
      waits.push(ms)
      return real(fn, 0)
    })
    return waits
  }

  /** A job that answers `compressing` `n` times before it completes. */
  function jobTaking(n) {
    const responses = [json({ job_id: 'j' })]
    for (let i = 0; i < n; i += 1) responses.push(json({ status: 'compressing' }))
    responses.push(json({ status: 'completed' }))
    responses.push(json({})) // download
    responses.push(json({})) // delete
    return fetcherFrom(responses)
  }

  /**
   * Issue #48, the half this file owns. The loop slept a flat 400 ms before
   * asking a second time, so a job the server had already finished cost the
   * browser that much, and twice what the CLI cost after the Rust half was
   * fixed.
   */
  it('does not make a job that finishes at once wait out the ceiling', async () => {
    const waits = captureDelays()
    const { fetcher } = jobTaking(1)

    await compress(
      { body: new Blob(['x']), name: 'a.txt', algorithm: 'zip', level: 3 },
      { fetcher },
    )

    expect(waits).toEqual([FIRST_POLL_DELAY])
    expect(FIRST_POLL_DELAY).toBeLessThan(400)
  })

  it('doubles to the ceiling and then holds', async () => {
    const waits = captureDelays()
    const { fetcher } = jobTaking(8)

    await compress(
      { body: new Blob(['x']), name: 'a.txt', algorithm: 'zip', level: 3 },
      { fetcher },
    )

    expect(waits).toEqual([10, 20, 40, 80, 160, 200, 200, 200])
    expect(Math.max(...waits)).toBe(MAX_POLL_DELAY)
  })

  /**
   * The two clients must not drift. The browser waiting longer than the CLI
   * for the same job is exactly what this issue was about.
   */
  it('keeps the same schedule the Rust client uses', () => {
    expect(FIRST_POLL_DELAY).toBe(10)
    expect(MAX_POLL_DELAY).toBe(200)
    expect(nextDelay(FIRST_POLL_DELAY)).toBe(20)
    expect(nextDelay(160)).toBe(MAX_POLL_DELAY)
    expect(nextDelay(MAX_POLL_DELAY)).toBe(MAX_POLL_DELAY)
    expect(nextDelay(10_000)).toBe(MAX_POLL_DELAY)
  })
})
