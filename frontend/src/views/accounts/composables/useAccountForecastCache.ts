import type { AccountQuotaForecastResponse } from '@/api'
import { onScopeDispose, shallowRef } from 'vue'
import { getAccountQuotaForecast } from '@/api'

export const FORECAST_TTL_MS = 10_000
const FAILURE_TTL_MS = 30_000
const MAX_ENTRIES = 200
const MAX_CONCURRENT = 2

interface Entry {
  report: AccountQuotaForecastResponse | null
  expiresAt: number
}

interface Job {
  id: string
  controller: AbortController
  listeners: Set<(report: AccountQuotaForecastResponse | null) => void>
}

export type AccountForecastCache = ReturnType<typeof useAccountForecastCache>

/** Page-owned cache: subscribers share reads, but cancellation remains per consumer. */
export function useAccountForecastCache() {
  const entries = shallowRef(new Map<string, Entry>())
  const jobs = new Map<string, Job>()
  const signatures = new Map<string, string>()
  const queue: Job[] = []
  let active = 0
  let disposed = false

  function remember(id: string, report: AccountQuotaForecastResponse | null) {
    const next = new Map(entries.value)
    next.delete(id)
    while (next.size >= MAX_ENTRIES)
      next.delete(next.keys().next().value!)
    next.set(id, { report, expiresAt: Date.now() + (report ? FORECAST_TTL_MS : FAILURE_TTL_MS) })
    entries.value = next
  }

  function finish(job: Job, report: AccountQuotaForecastResponse | null) {
    if (jobs.get(job.id) === job)
      jobs.delete(job.id)
    for (const resolve of [...job.listeners])
      resolve(report)
    job.listeners.clear()
  }

  function cancel(job: Job) {
    job.controller.abort()
    const index = queue.indexOf(job)
    if (index >= 0)
      queue.splice(index, 1)
    finish(job, null)
  }

  async function execute(job: Job) {
    try {
      const result = await getAccountQuotaForecast({ accountId: job.id }, {
        signal: job.controller.signal,
        silent: true,
      })
      if (!disposed && !job.controller.signal.aborted && jobs.get(job.id) === job) {
        const report = result.accountId === job.id ? result : null
        remember(job.id, report)
        finish(job, report)
      }
    }
    catch {
      if (!disposed && !job.controller.signal.aborted && jobs.get(job.id) === job) {
        remember(job.id, null)
        finish(job, null)
      }
    }
    finally {
      active -= 1
      pump()
    }
  }

  function pump() {
    if (disposed)
      return
    while (active < MAX_CONCURRENT && queue.length) {
      const job = queue.shift()!
      if (job.controller.signal.aborted || !job.listeners.size)
        continue
      active += 1
      void execute(job)
    }
  }

  function read(id: string, signal?: AbortSignal): Promise<AccountQuotaForecastResponse | null> {
    if (disposed || !id || signal?.aborted)
      return Promise.resolve(null)
    const cached = entries.value.get(id)
    if (cached && cached.expiresAt > Date.now())
      return Promise.resolve(cached.report)
    let job = jobs.get(id)
    if (!job) {
      job = { id, controller: new AbortController(), listeners: new Set() }
      jobs.set(id, job)
      queue.push(job)
    }
    const current = job
    return new Promise((resolve) => {
      function done(report: AccountQuotaForecastResponse | null) {
        signal?.removeEventListener('abort', abort)
        current.listeners.delete(done)
        resolve(report)
      }
      function abort() {
        done(null)
        if (!current.listeners.size)
          cancel(current)
      }
      current.listeners.add(done)
      signal?.addEventListener('abort', abort, { once: true })
      pump()
    })
  }

  function invalidate(id: string) {
    const job = jobs.get(id)
    if (job)
      cancel(job)
    const next = new Map(entries.value)
    next.delete(id)
    entries.value = next
  }

  function peek(id: string, now: number) {
    const entry = entries.value.get(id)
    return entry && entry.expiresAt > now ? entry.report : null
  }

  function syncAccount(account: Parameters<typeof accountForecastSignature>[0] & { id: string }) {
    const signature = accountForecastSignature(account)
    const previous = signatures.get(account.id)
    if (previous !== undefined && previous !== signature)
      invalidate(account.id)
    signatures.delete(account.id)
    while (signatures.size >= MAX_ENTRIES)
      signatures.delete(signatures.keys().next().value!)
    signatures.set(account.id, signature)
  }

  onScopeDispose(() => {
    disposed = true
    for (const job of [...jobs.values()])
      cancel(job)
    entries.value = new Map()
    signatures.clear()
  })

  return { read, peek, invalidate, syncAccount }
}

export function accountForecastSignature(account: {
  planType: string | null
  quota: { windows: { key: string, resetAt?: string | null, windowSeconds: number | null }[] }
}) {
  return JSON.stringify([account.planType, account.quota.windows.map(window =>
    [window.key, window.resetAt, window.windowSeconds])])
}
