// The backend's job flow, from a browser.
//
// Every call is same-origin: in production nginx proxies these paths to the
// backend, and in development Vite does. That is deliberate, because the
// backend ships no CORS layer and should not need one.

/** How often a running job is polled. */
export const POLL_INTERVAL = 400

/** Read the backend's error shape, falling back to the status line. */
async function failure(response) {
  let detail = ''
  try {
    detail = (await response.json())?.detail ?? ''
  } catch {
    /* not the JSON error shape */
  }
  return new Error(
    detail
      ? `the server rejected the request (HTTP ${response.status}): ${detail}`
      : `the server rejected the request (HTTP ${response.status})`,
  )
}

/** Is the server there, and is it a Collapse one? */
export async function health(fetcher = fetch) {
  const response = await fetcher('/health')
  if (!response.ok) throw await failure(response)
  const body = await response.json()
  if (body?.status !== 'ok') throw new Error('that address is not a Collapse server')
  return true
}

/**
 * What the client should do with a job it just polled. Mirrors the decision
 * the Rust client makes: only the two in-progress states mean "wait", so a
 * server answering anything else stops the loop instead of spinning forever.
 */
export function progressOf(job) {
  switch (job?.status) {
    case 'queued':
    case 'compressing':
      return 'waiting'
    case 'completed':
      return 'ready'
    case 'failed':
      throw new Error(job.error_message || 'compression failed on the server')
    case undefined:
      throw new Error('malformed server response: no status')
    default:
      throw new Error(`unexpected job status from the server: ${job.status}`)
  }
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

/**
 * Run one compression end to end: queue it, poll until it settles, download
 * the archive, then delete the job so the server keeps nothing.
 *
 * `body` is what to upload (a File, or a tar Blob for a folder). `onStatus` is
 * called with every status the job passes through, which is what lets the UI
 * show the flow rather than a single spinner.
 */
export async function compress(
  { body, name, algorithm, level, envelope = 'none' },
  { onStatus = () => {}, fetcher = fetch } = {},
) {
  const query = new URLSearchParams({ name, algorithm, level: String(level), envelope })

  onStatus('uploading')
  const accepted = await fetcher(`/compress?${query}`, { method: 'POST', body })
  if (!accepted.ok) throw await failure(accepted)
  const job = await accepted.json()
  if (!job?.job_id) throw new Error('malformed server response: no job_id')

  let last = null
  for (;;) {
    const polled = await fetcher(`/jobs/${job.job_id}`)
    if (!polled.ok) throw await failure(polled)
    const current = await polled.json()

    if (current.status !== last) {
      last = current.status
      onStatus(current.status)
    }
    if (progressOf(current) === 'ready') break
    await sleep(POLL_INTERVAL)
  }

  onStatus('downloading')
  const download = await fetcher(`/jobs/${job.job_id}/download`)
  if (!download.ok) throw await failure(download)
  const archive = await download.blob()

  // Best effort: the archive is already in hand, so a failed delete must not
  // fail the operation.
  try {
    await fetcher(`/jobs/${job.job_id}`, { method: 'DELETE' })
  } catch {
    /* the job will be cleaned up with the server */
  }

  onStatus('done')
  return { archive, name: job.archive_name }
}
