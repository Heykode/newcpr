/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { setImmediate } from 'node:timers/promises'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

function mountDashboard() {
  const pending = []
  let refresh
  let dispose
  const dependencies = {
    'vue': { ...vue, onMounted: callback => callback(), onScopeDispose: (callback) => { dispose = callback } },
    '@lucide/vue': { Activity: {}, FileText: {}, Timer: {}, Users: {} },
    '@vueuse/core': { useIntervalFn: (callback, interval) => {
      assert.equal(interval, 30_000)
      refresh = callback
      return { resume() {} }
    } },
    '@/api': {
      getDashboardSummary: (_query, options) => new Promise((resolve, reject) => pending.push({ resolve, reject, ...options })),
      getDashboardTrend: () => assert.fail('unexpected trend request'),
    },
    '@/utils/async': { errorMessage: error => error.message, withMinimumDuration: action => action() },
    '@/utils/date': { formatDateTime: () => 'fixture-time' },
    '@/utils/number': { formatCompactNumber: String, formatInteger: String },
  }
  const source = readFileSync(new URL('../src/views/dashboard/composables/useDashboard.ts', import.meta.url), 'utf8')
  const { outputText } = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 } })
  const exports = {}
  runInNewContext(outputText, { exports, AbortController, require: (name) => {
    assert.ok(name in dependencies, name)
    return dependencies[name]
  } })
  const state = exports.useDashboard()
  return { state, pending, autoRefresh: () => refresh(), dispose: () => dispose() }
}

function summary(marker) {
  return { trend: { kind: 'usage', points: [] }, usageRecords: [{ marker }] }
}

test('silent dashboard refresh preserves content and deduplicates in-flight summaries', async () => {
  const view = mountDashboard()
  try {
    assert.equal(view.state.loading.value, true)
    view.pending[0].resolve(summary('initial'))
    await setImmediate()
    view.autoRefresh()
    view.autoRefresh()
    assert.equal(view.pending.length, 2)
    assert.equal(view.pending[1].silent, true)
    assert.equal(view.state.loading.value, false)
    assert.equal(view.state.trendLoading.value, false)
    assert.equal(view.state.usageRecords.value[0].marker, 'initial')
    view.pending[1].reject(new Error('temporary refresh failure'))
    await setImmediate()
    assert.equal(view.state.trendError.value, '')
    assert.equal(view.state.usageRecords.value[0].marker, 'initial')
    view.autoRefresh()
    view.pending[2].resolve(summary('updated'))
    await setImmediate()
    assert.equal(view.state.usageRecords.value[0].marker, 'updated')
  }
  finally { view.dispose() }
})

test('manual errors remain visible through silent retries until a successful snapshot', async () => {
  const view = mountDashboard()
  try {
    view.pending[0].resolve(summary('initial'))
    await setImmediate()
    const manual = view.state.refresh()
    assert.equal(view.state.refreshing.value, true)
    assert.equal(view.state.trendLoading.value, true)
    view.pending[1].reject(new Error('manual error'))
    await manual
    assert.equal(view.state.trendError.value, 'manual error')
    view.autoRefresh()
    assert.equal(view.state.trendError.value, 'manual error')
    view.pending[2].reject(new Error('silent error'))
    await setImmediate()
    assert.equal(view.state.trendError.value, 'manual error')
    view.autoRefresh()
    view.pending[3].resolve(summary('recovered'))
    await setImmediate()
    assert.equal(view.state.trendError.value, '')
  }
  finally { view.dispose() }
})

test('disposing the dashboard aborts refresh and ignores its late result', async () => {
  const view = mountDashboard()
  view.pending[0].resolve(summary('initial'))
  await setImmediate()
  view.autoRefresh()
  view.dispose()
  assert.equal(view.pending[1].signal.aborted, true)
  view.pending[1].resolve(summary('late'))
  await setImmediate()
  assert.equal(view.state.usageRecords.value[0].marker, 'initial')
})
