/* eslint-disable test/no-import-node-test -- Node regression runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

const require = createRequire(import.meta.url)
function harness() {
  const modules = new Map()
  const calls = []
  const route = vue.reactive({ query: {} })
  const router = { replace: async ({ query }) => {
    route.query = query
  } }
  const api = Object.fromEntries([
    'getUsageRecords',
    'getUsageRecordSummary',
    'getUsageRecordInsightsOverview',
    'getUsageRecordInsightsDiagnostics',
    'getOpsErrors',
  ].map(name => [name, (query, options) => new Promise((resolve, reject) => calls.push({ name, query, ...options, resolve, reject }))]))
  function load(filename) {
    if (modules.has(filename.href))
      return modules.get(filename.href)
    const exports = {}
    modules.set(filename.href, exports)
    const { outputText } = ts.transpileModule(readFileSync(filename, 'utf8'), {
      compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
    })
    runInNewContext(outputText, {
      exports,
      AbortController,
      require(name) {
        if (name === '@/api')
          return api
        if (name === 'vue')
          return { ...vue, onMounted: () => {} }
        if (name === 'vue-router')
          return { useRoute: () => route, useRouter: () => router }
        if (name === '@vueuse/core')
          return { watchDebounced: (source, fn) => vue.watch(source, fn, { flush: 'post' }) }
        if (name === '@/utils/async')
          return { withMinimumDuration: action => action() }
        if (name.startsWith('@/') || name.startsWith('.')) {
          return load(name.startsWith('@/')
            ? new URL(`../src/${name.slice(2)}.ts`, import.meta.url)
            : new URL(`${name}.ts`, filename))
        }
        return require(name)
      },
    }, { filename: filename.pathname })
    return exports
  }
  const utils = load(new URL('../src/views/usage/utils/filters.ts', import.meta.url))
  const table = load(new URL('../src/views/usage/composables/useUsageRecordsTable.ts', import.meta.url))
  const filters = load(new URL('../src/views/usage/composables/useUsageFilters.ts', import.meta.url))
  return { utils, table, filters, calls, route }
}
async function flush() {
  await vue.nextTick()
  await Promise.resolve()
  await vue.nextTick()
}
const range = { startTime: '2026-09-01T00:00:00Z', endTime: '2026-09-02T00:00:00Z' }

test('shared filter validation rejects bad ranges and hides credentials and view-only keys', () => {
  const { utils } = harness()
  const { usageFilterParams, usageFilterError, readUsageFilterDraft } = utils
  assert.ok(usageFilterError({ minLatencyMs: '2', maxLatencyMs: '1' }))
  assert.ok(usageFilterError({ minLatencyMs: 'Infinity' }))
  assert.ok(usageFilterError({ clientStatusCode: '600' }))
  assert.ok(usageFilterError({ minFirstTokenMs: '1e2' }))
  assert.ok(usageFilterError({ upstreamMode: 'guess' }))
  assert.ok(usageFilterError({ accountIds: Array.from({ length: 51 }, (_, i) => `${i}`).join(',') }))
  assert.ok(usageFilterError({ startTime: range.startTime }))
  const draft = readUsageFilterDraft({ accountIds: 'a,b', search: 'sk_fake-secret-value', view: 'errors', timeRange: '7d', password: 'ignored', clientStatusCode: ['400', '500'] })
  assert.equal(draft.search, 'sk_fake-se')
  assert.equal(draft.password, undefined)
  assert.equal(draft.clientStatusCode, undefined)
  const params = usageFilterParams({ ...draft, clientStatusCode: '502', errorCode: 'blocked' })
  assert.equal(params.clientStatusCode, 502)
  assert.equal(params.view, undefined)
  assert.equal(params.errorCode, undefined)
  assert.equal(usageFilterParams({ errorCode: 'blocked' }, true).errorCode, 'blocked')
})

test('opaque response IDs retain their original length through filters and URL restore', () => {
  const { utils } = harness()
  const responseId = `resp_${'x'.repeat(4096)}\0tail`
  const restored = utils.readUsageFilterDraft({ responseId })
  assert.equal(utils.usageFilterError(restored), '')
  assert.equal(restored.responseId, responseId)
  assert.equal(utils.usageFilterParams(restored).responseId, responseId)
  assert.ok(utils.usageFilterError({ requestedModel: 'x'.repeat(257) }))
})

test('URL restore/back and invalid drafts retain the last valid API filters', async () => {
  const { filters, route } = harness()
  route.query = { accountIds: 'account-old', view: 'errors', search: 'sk_fake-secret-value' }
  const scope = vue.effectScope()
  const state = scope.run(() => filters.useUsageFilters())
  try {
    await flush()
    assert.equal(route.query.search, 'sk_fake-se')
    assert.equal(state.filters.value.accountIds, 'account-old')
    state.draft.value = { ...state.draft.value, minLatencyMs: '-1' }
    await flush()
    assert.ok(state.error.value)
    assert.equal(state.filters.value.minLatencyMs, undefined)
    state.draft.value = { accountIds: 'account-new', clientStatusCode: '502' }
    await flush()
    assert.equal(route.query.clientStatusCode, '502')
    route.query = { accountIds: 'account-old', view: 'errors' }
    await flush()
    assert.equal(state.draft.value.accountIds, 'account-old')
    assert.equal(state.filters.value.clientStatusCode, undefined)
  }
  finally {
    scope.stop()
  }
})

test('table/summary/overview/diagnostics use one frozen range and filter snapshot', async () => {
  const { table, calls } = harness()
  const scope = vue.effectScope()
  let clock = 0
  const filters = vue.ref({ accountIds: 'acct-a', search: 'literal_%', upstreamMode: 'excel' })
  const state = scope.run(() => table.useUsageRecordsTable({
    timeRangeParams: vue.ref(range),
    latestTimeRangeParams: () => ({ ...range, endTime: `2026-09-02T00:00:0${++clock}Z` }),
    active: vue.ref(true),
    filters,
  }))
  try {
    const pending = state.loadUsageRecords()
    assert.equal(calls.length, 4)
    assert.equal(new Set(calls.map(call => call.query.endTime)).size, 1)
    for (const call of calls) {
      assert.equal(call.query.accountIds, 'acct-a')
      assert.equal(call.query.search, 'literal_%')
      assert.equal(call.query.upstreamMode, 'excel')
      call.resolve(call.name === 'getUsageRecords'
        ? { items: [{ id: 'old' }], currentPage: 1, pageSize: 10, total: 1 }
        : { dimension: 'model', currentPage: 1, items: [], hasMore: false })
    }
    await pending
    state.handlePageChange(2)
    const stale = calls.at(-1)
    assert.equal(stale.query.endTime, calls[0].query.endTime)
    filters.value = { accountIds: 'acct-b', clientStatusCode: 200 }
    assert.equal(stale.signal.aborted, true)
    assert.equal(state.records.value.length, 0)
    await flush()
    const fresh = calls.slice(5)
    assert.equal(fresh.length, 4)
    assert.equal(new Set(fresh.map(call => call.query.endTime)).size, 1)
    for (const call of fresh) {
      assert.equal(call.query.accountIds, 'acct-b')
      assert.equal(call.query.search, undefined)
      call.resolve(call.name === 'getUsageRecords'
        ? { items: [{ id: 'new' }], currentPage: 1, pageSize: 10, total: 1 }
        : { dimension: 'model', currentPage: 1, items: [], hasMore: false })
    }
    await flush()
    stale.resolve({ items: [{ id: 'stale' }], currentPage: 2, pageSize: 10, total: 1 })
    await flush()
    assert.equal(state.records.value[0].id, 'new')
    assert.equal(state.currentPage.value, 1)
  }
  finally {
    scope.stop()
  }
})
