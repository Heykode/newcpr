/* eslint-disable test/no-import-node-test -- regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

const require = createRequire(import.meta.url)
const tick = () => new Promise(resolve => setImmediate(resolve))

function load(path, dependencies, globals = {}) {
  const file = new URL(`../src/views/accounts/${path}.ts`, import.meta.url)
  const exports = {}
  const { outputText } = ts.transpileModule(readFileSync(file, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, {
    exports,
    require: name => dependencies[name] ?? require(name),
    AbortController,
    ...globals,
  }, { filename: file.pathname })
  return exports
}

function harness(t) {
  let time = Date.parse('2026-09-14T00:00:00Z')
  const clock = vue.ref(new Date(time))
  const pending = []
  const api = {
    getAccountQuotaForecast: ({ accountId }, options) => new Promise((resolve, reject) => {
      pending.push({ accountId, ...options, resolve, reject })
    }),
  }
  const cacheModule = load('composables/useAccountForecastCache', { vue, '@/api': api }, {
    Date: class extends Date { static now() { return time } },
  })
  const scope = vue.effectScope()
  t.after(() => scope.stop())
  const cache = scope.run(() => cacheModule.useAccountForecastCache())
  return {
    pending,
    scope,
    cache,
    cacheModule,
    clock,
    advance(ms) {
      time += ms
      clock.value = new Date(time)
    },
    now: () => time,
  }
}

const report = accountId => ({ accountId, forecasts: [] })

test('same-account readers share one request; cancellation belongs to each subscriber', async (t) => {
  const h = harness(t)
  const a = new AbortController()
  const b = new AbortController()
  const first = h.cache.read('a', a.signal)
  const second = h.cache.read('a', b.signal)
  assert.equal(h.pending.length, 1)
  a.abort()
  assert.equal(await first, null)
  assert.equal(h.pending[0].signal.aborted, false)
  h.pending[0].resolve(report('a'))
  assert.equal((await second).accountId, 'a')
  assert.equal((await h.cache.read('a')).accountId, 'a')
  assert.equal(h.pending.length, 1)
  assert.equal(h.pending[0].silent, true)
})

test('queue never exceeds two in flight; cancelled queue entries do not run', async (t) => {
  const h = harness(t)
  const controllers = Array.from({ length: 5 }, () => new AbortController())
  const tasks = controllers.map((controller, i) => h.cache.read(String(i), controller.signal))
  assert.equal(h.pending.length, 2)
  controllers[2].abort()
  h.pending[0].resolve(report('0'))
  await tick()
  assert.deepEqual(h.pending.map(item => item.accountId), ['0', '1', '3'])
  h.pending[1].resolve(report('1'))
  await tick()
  assert.equal(h.pending[3].accountId, '4')
  h.pending[2].resolve(report('3'))
  h.pending[3].resolve(report('4'))
  assert.equal((await Promise.all(tasks))[2], null)
})

test('invalidated or disposed work cannot overwrite a newer account report', async (t) => {
  const h = harness(t)
  const old = h.cache.read('a')
  h.cache.invalidate('a')
  assert.equal(await old, null)
  assert.equal(h.pending[0].signal.aborted, true)
  const next = h.cache.read('a')
  h.pending[1].resolve({ ...report('a'), generatedAt: 'new' })
  await next
  h.pending[0].resolve({ ...report('a'), generatedAt: 'old' })
  await tick()
  assert.equal(h.cache.peek('a', h.now()).generatedAt, 'new')
  const stopped = h.cache.read('b')
  h.scope.stop()
  assert.equal(await stopped, null)
  h.pending[2].resolve(report('b'))
  await tick()
  assert.equal(h.cache.peek('b', h.now()), null)
})

test('unknown failures back off, wrong-account replies are rejected, and cache is bounded', async (t) => {
  const h = harness(t)
  const failed = h.cache.read('a')
  h.pending[0].reject(new Error('fixture'))
  assert.equal(await failed, null)
  assert.equal(await h.cache.read('a'), null)
  assert.equal(h.pending.length, 1)
  h.advance(30_001)
  const retry = h.cache.read('a')
  h.pending[1].resolve(report('b'))
  assert.equal(await retry, null)
  for (let i = 0; i < 201; i++) {
    const task = h.cache.read(`cache-${i}`)
    h.pending.at(-1).resolve(report(`cache-${i}`))
    await task
    await tick()
  }
  assert.equal(h.cache.peek('cache-0', h.now()), null)
  assert.equal(h.cache.peek('cache-200', h.now()).accountId, 'cache-200')
  h.advance(300_001)
  assert.equal(h.cache.peek('cache-200', h.now()), null)
})

test('list refreshes only synchronize detail cache boundaries and never prefetch learned forecasts', async (t) => {
  const h = harness(t)
  const module = load('composables/useAccountListForecast', {
    vue,
    './useAccountForecastCache': h.cacheModule,
  })
  const account = resetAt => ({
    id: 'a',
    planType: 'plus',
    quota: { windows: [{ key: 'week', resetAt, windowSeconds: 604800 }] },
  })
  const accounts = vue.ref([account('2026-09-20T00:00:00Z')])
  const state = h.scope.run(() => module.useAccountListForecast(accounts))
  assert.equal(h.pending.length, 0)
  const opened = state.cache.read('a')
  h.pending[0].resolve(report('a'))
  await opened
  for (let i = 0; i < 4; i++) {
    accounts.value = [account('2026-09-20T00:00:00Z')]
    h.advance(30_000)
    await tick()
  }
  assert.equal(h.pending.length, 1)
  h.advance(181_000)
  await tick()
  assert.equal(h.pending.length, 1)
  assert.equal(state.cache.peek('a', h.now()), null)
  const reopened = state.cache.read('a')
  h.pending[1].resolve(report('a'))
  await reopened
  accounts.value = [account('2026-09-27T00:00:00Z')]
  await tick()
  assert.equal(h.pending.length, 2)
  assert.equal(state.cache.peek('a', h.now()), null)
})

test('weekly summary displays the backend current-window estimate without recomputing or learning', () => {
  const { weeklyForecastPresentation: present } = load('components/AccountQuotaSummaryCell/forecast', {})
  const now = Date.parse('2026-09-14T00:00:00Z')
  const base = {
    quotaWindow: { key: 'week', period: 'weekly', usedPercent: 1, estimatedUsd: 200, resetAt: '2026-09-20T00:00:00Z' },
    costs: [{ currency: 'USD', estimatedAmount: '20', estimatedAmountDisplay: '$999.00' }],
    costEstimateStatus: 'partial',
  }
  const view = value => present(value, now)
  assert.equal(view(base).amount, '$200.00')
  assert.doesNotMatch(view(base).amount, /≈/)
  assert.match(view(base).title, /费用记录不完整/)
  for (const estimatedUsd of [null, undefined, 0, -1, Number.NaN, Number.POSITIVE_INFINITY, '10'])
    assert.equal(view({ ...base, quotaWindow: { ...base.quotaWindow, estimatedUsd } }).amount, '—')
  assert.equal(view({ ...base, costs: [] }).amount, '$200.00')
  for (const resetAt of [null, 'invalid', new Date(now).toISOString()])
    assert.equal(view({ ...base, quotaWindow: { ...base.quotaWindow, resetAt } }).amount, '—')
  assert.equal(view({ ...base, quotaWindow: { ...base.quotaWindow, period: 'monthly' } }).amount, '—')
  assert.equal(view({ ...base, quotaWindow: null }).amount, '—')
  assert.equal(view({ ...base, quotaWindow: undefined }).amount, '—')
  assert.equal(view({ ...base, quotaWindow: { ...base.quotaWindow, estimatedUsd: 1.2345 } }).amount, '$1.23')
})

test('modal and list share manual refresh even when the quota reset boundary changes', async (t) => {
  const h = harness(t)
  const listModule = load('composables/useAccountListForecast', {
    vue,
    './useAccountForecastCache': h.cacheModule,
  })
  const first = {
    id: 'a',
    planType: 'plus',
    quota: { windows: [{ key: 'week', resetAt: '2026-09-20T00:00:00Z', windowSeconds: 604800 }] },
  }
  const updated = {
    ...first,
    quota: { windows: [{ ...first.quota.windows[0], resetAt: '2026-09-27T00:00:00Z' }] },
  }
  const accounts = vue.ref([first])
  const list = h.scope.run(() => listModule.useAccountListForecast(accounts))
  const firstRead = list.cache.read('a')
  h.pending[0].resolve(report('a'))
  await firstRead
  const modalModule = load('composables/useAccountQuotaForecast', {
    vue,
    '@/api': { refreshAccountQuota: async () => ({ account: updated }) },
    '@/components/base/BaseToast': { toast: { success() {} } },
  }, { Date: class extends Date { static now() { return h.now() } } })
  const modal = h.scope.run(() => modalModule.useAccountQuotaForecast(
    vue.ref('a'),
    vue.ref(true),
    account => accounts.value = [account],
    list.cache,
  ))
  await tick()
  assert.equal(h.pending.length, 1)
  assert.equal(modal.report.value.accountId, 'a')
  const refresh = modal.refresh()
  await tick()
  // The list may cancel the old boundary, but the modal must join the current read.
  for (const pending of h.pending.slice(1))
    pending.resolve(report('a'))
  await tick()
  for (const pending of h.pending.slice(1))
    pending.resolve(report('a'))
  await refresh
  assert.equal(modal.error.value, false)
  assert.equal(modal.report.value?.accountId, 'a')
  assert.equal(list.cache.peek('a', h.now())?.accountId, 'a')
  list.cache.invalidate('a')
  const background = list.cache.read('a')
  h.pending.at(-1).resolve({ ...report('a'), generatedAt: 'background-update' })
  await background
  await tick()
  assert.equal(modal.report.value.generatedAt, 'background-update')
})
