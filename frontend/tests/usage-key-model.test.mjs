/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'
import { compileScript, parse } from 'vue/compiler-sfc'
import { renderToString } from 'vue/server-renderer'

const require = createRequire(import.meta.url)
const calls = []
const api = {
  getUsageRecords: async () => ({ items: [], currentPage: 1, pageSize: 10, total: 0 }),
  getUsageRecordSummary: async () => ({}),
  getUsageRecordInsightsOverview: async () => ({}),
  getUsageRecordInsightsDiagnostics: (query, options) =>
    new Promise((resolve, reject) => calls.push({ query, ...options, resolve, reject })),
}

const icon = vue.defineComponent({ setup: () => () => vue.h('svg') })
const card = vue.defineComponent({
  setup: (_, { slots }) => () => vue.h('article', [slots.actions?.(), slots.body?.()]),
})
const table = vue.defineComponent({
  props: ['rows', 'columns'],
  setup: (props, { slots }) => () => vue.h('table', props.rows.map(row =>
    vue.h('tr', { 'data-key': row.key }, props.columns.map(column =>
      vue.h('td', slots[column.key]?.({ row }) ?? String(row[column.key])))))),
})
const button = vue.defineComponent({
  props: ['label', 'disabled'],
  setup: (props, { slots }) => () => vue.h('button', {
    'aria-label': props.label,
    'disabled': props.disabled,
  }, slots.default?.()),
})
const select = vue.defineComponent({
  props: ['options'],
  setup: props => () => vue.h('select', props.options.map(option =>
    vue.h('option', { value: option.value }, option.label))),
})
const modules = new Map()
function loadSource(filename) {
  if (modules.has(filename.href))
    return modules.get(filename.href)
  const exports = {}
  modules.set(filename.href, exports)
  const source = readFileSync(filename, 'utf8')
  const content = filename.pathname.endsWith('.vue')
    ? compileScript(parse(source, { filename: filename.pathname }).descriptor, {
      id: filename.pathname,
      inlineTemplate: true,
    }).content
    : source
  const { outputText } = ts.transpileModule(content, {
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
      if (name === '@vueuse/core')
        return { watchDebounced: vue.watch }
      if (name === '@lucide/vue')
        return { ChevronLeft: icon, ChevronRight: icon, CornerDownRight: icon }
      if (name === '@/utils/async')
        return { withMinimumDuration: action => action() }
      if (name === '@/utils/providers')
        return { formatProviderLabel: value => value }
      if (name === '@/components/base/BaseCard.vue')
        return card
      if (name === '@/components/base/BaseTable/index.vue')
        return table
      if (name === '@/components/base/BaseTable/columns')
        return { defineTableColumns: columns => columns }
      if (name === '@/components/base/BaseIconButton.vue')
        return button
      if (name === '@/components/base/BaseSelect.vue')
        return select
      if (name === '@/components/base/BaseEmpty.vue')
        return vue.defineComponent({ props: ['title'], setup: props => () => vue.h('p', props.title) })
      if (name.startsWith('@/') || name.startsWith('.')) {
        const path = name.endsWith('.vue') ? name : `${name}.ts`
        return loadSource(name.startsWith('@/')
          ? new URL(`../src/${path.slice(2)}`, import.meta.url)
          : new URL(path, filename))
      }
      return require(name)
    },
  }, { filename: filename.pathname })
  return exports
}

const { useUsageRecordsTable } = loadSource(new URL('../src/views/usage/composables/useUsageRecordsTable.ts', import.meta.url))
const component = loadSource(new URL('../src/views/usage/components/UsageDiagnosticCard.vue', import.meta.url)).default
const range = { startTime: '2026-09-01T00:00:00Z', endTime: '2026-09-02T00:00:00Z' }
function harness() {
  calls.length = 0
  const timeRangeParams = vue.ref({ ...range })
  const scope = vue.effectScope()
  const state = scope.run(() => useUsageRecordsTable({
    timeRangeParams,
    latestTimeRangeParams: () => timeRangeParams.value,
    active: vue.ref(true),
  }))
  return { state, scope, timeRangeParams }
}
function page(currentPage, hasMore = false, dimension = 'keyModel', items = []) {
  return { dimension, items, currentPage, pageSize: dimension === 'keyModel' ? 20 : 100, hasMore }
}
async function flush() {
  await vue.nextTick()
  await Promise.resolve()
  await vue.nextTick()
}

test('Key/model uses bounded next/previous pages without reloading other analytics', async () => {
  const { state, scope } = harness()
  try {
    state.diagnosticDimension.value = 'keyModel'
    await flush()
    assert.equal(calls[0].query.currentPage, 1)
    assert.equal(calls[0].query.pageSize, 20)
    calls[0].resolve(page(1, true))
    await flush()
    assert.equal(state.diagnosticLoading.value, false)
    state.handleDiagnosticPageChange(2)
    await flush()
    assert.equal(calls[1].query.currentPage, 2)
    assert.equal(state.diagnosticLoading.value, true)
    state.handleDiagnosticPageChange(3)
    assert.equal(calls.length, 2, 'no concurrent page action while loading')
    calls[1].resolve(page(2))
    await flush()
    state.handleDiagnosticPageChange(3)
    state.handleDiagnosticPageChange(0)
    state.handleDiagnosticPageChange(1.5)
    assert.equal(calls.length, 2, 'no out-of-range pages')
    state.handleDiagnosticPageChange(1)
    await flush()
    calls[2].resolve(page(1, true))
    await flush()
    assert.equal(state.insights.value.diagnostics.currentPage, 1)
  }
  finally {
    scope.stop()
  }
})

test('dimension changes cancel stale pages and only Key/model sends pagination', async () => {
  const { state, scope } = harness()
  try {
    state.diagnosticDimension.value = 'keyModel'
    await flush()
    const old = calls[0]
    state.diagnosticDimension.value = 'account'
    await flush()
    assert.equal(old.signal.aborted, true)
    assert.equal(calls[1].query.currentPage, undefined)
    assert.equal(calls[1].query.pageSize, undefined)
    calls[1].resolve(page(1, false, 'account'))
    await flush()
    old.resolve(page(4, true))
    await flush()
    assert.equal(state.insights.value.diagnostics.dimension, 'account')
    assert.equal(state.diagnosticLoading.value, false)
    state.diagnosticDimension.value = 'keyModel'
    await flush()
    assert.equal(calls[2].query.currentPage, 1)
    scope.stop()
    assert.equal(calls[2].signal.aborted, true)
    calls[2].resolve(page(1))
    await flush()
    assert.equal(state.insights.value.diagnostics.dimension, 'account')
  }
  finally {
    scope.stop()
  }
})

test('time/provider refresh resets page and owns loading after replacing a pending read', async () => {
  const { state, scope, timeRangeParams } = harness()
  try {
    state.diagnosticDimension.value = 'keyModel'
    await flush()
    calls[0].resolve(page(1, true))
    await flush()
    state.handleDiagnosticPageChange(2)
    await flush()
    const pending = calls[1]
    timeRangeParams.value = { ...range, endTime: '2026-09-03T00:00:00Z' }
    const refresh = state.loadUsageRecords()
    await flush()
    assert.equal(pending.signal.aborted, true)
    assert.equal(calls[2].query.currentPage, 1)
    assert.equal(calls[2].query.endTime, timeRangeParams.value.endTime)
    calls[2].resolve(page(1, true))
    await refresh
    pending.resolve(page(2))
    await flush()
    assert.equal(state.insights.value.diagnostics.currentPage, 1)
    assert.equal(state.diagnosticLoading.value, false)
    assert.equal(state.analyticsLoading.value, false)
    state.handleDiagnosticPageChange(2)
    await flush()
    calls[3].reject(new Error('synthetic error'))
    await flush()
    assert.equal(state.diagnosticLoading.value, false)
    state.providerQuery.value = 'openai'
    await flush()
    assert.equal(calls[4].query.currentPage, 1)
    assert.equal(calls[4].query.provider, 'openai')
    assert.equal(state.diagnosticLoading.value, true)
    state.handleDiagnosticPageChange(2)
    assert.equal(calls.length, 5, 'old pagination stays disabled during background filter refresh')
    calls[4].resolve(page(1))
    await flush()
  }
  finally {
    scope.stop()
  }
})

for (const filter of ['provider', 'time']) {
  test(`${filter} change cannot reuse old pagination after a failed first page`, async () => {
    const { state, scope, timeRangeParams } = harness()
    try {
      state.diagnosticDimension.value = 'keyModel'
      await flush()
      calls[0].resolve(page(1, true))
      await flush()
      for (const next of [2, 3]) {
        state.handleDiagnosticPageChange(next)
        await flush()
        calls.at(-1).resolve(page(next, true))
        await flush()
      }
      assert.equal(state.insights.value.diagnostics.currentPage, 3)
      if (filter === 'provider') {
        state.providerQuery.value = 'openai'
      }
      else {
        timeRangeParams.value = { ...range, endTime: '2026-09-03T00:00:00Z' }
      }
      state.handleDiagnosticPageChange(4)
      assert.equal(calls.length, 3, 'changed filters cannot use old pagination before watchers run')
      let refresh
      if (filter === 'time')
        refresh = state.loadUsageRecords()
      await flush()
      assert.equal(calls[3].query.currentPage, 1)
      calls[3].reject(new Error('synthetic filter refresh failure'))
      await refresh
      await flush()
      assert.equal(state.diagnosticLoading.value, false)
      const diagnostics = state.insights.value.diagnostics
      assert.equal(diagnostics.dimension, 'keyModel')
      assert.equal(diagnostics.currentPage, 1)
      assert.equal(diagnostics.hasMore, false)
      assert.equal(diagnostics.items.length, 0)
      for (const next of [2, 3, 4])
        state.handleDiagnosticPageChange(next)
      assert.equal(calls.length, 4, 'failed first page leaves no old next/previous page')

      const retry = state.loadUsageRecords()
      await flush()
      assert.equal(calls[4].query.currentPage, 1)
      assert.equal(calls[4].query.provider, filter === 'provider' ? 'openai' : undefined)
      assert.equal(calls[4].query.endTime, timeRangeParams.value.endTime)
      calls[4].resolve(page(1, true))
      await retry
      state.handleDiagnosticPageChange(2)
      await flush()
      assert.equal(calls[5].query.currentPage, 2)
      calls[5].resolve(page(2))
      await flush()
    }
    finally {
      scope.stop()
    }
  })
}

test('rendered Key/model page retains stable order, full names, tokens and partial cost', async () => {
  const item = (key, name, impactScore, estimatedCost) => ({
    key,
    name,
    impactScore,
    estimatedCost,
    costIncomplete: true,
    requestCount: 10,
    successCount: 10,
    errorCount: 0,
    errorRate: 0,
    requestShare: 0.5,
    nonCompletionCount: 0,
    nonCompletionRate: 0,
    retryCount: 0,
    retryRate: 0,
    firstTokenP95Ms: null,
    latencyP95Ms: null,
    totalTokens: 200,
    attemptCount: 10,
  })
  const longName = `Team → ${'long-key-name-'.repeat(8)}`
  const items = [
    item(JSON.stringify(['key-a', 'model-a']), `${longName} → model-a`, 0, '0'),
    item(JSON.stringify(['key-b', 'model-b']), 'Second → model-b', 1, null),
  ]
  const html = await renderToString(vue.createSSRApp(component, {
    dimension: 'keyModel',
    diagnostics: page(2, false, 'keyModel', items),
  }))
  assert.ok(html.indexOf(longName) < html.indexOf('Second'))
  assert.match(html, /model-a/)
  assert.match(html, /200/)
  assert.match(html, /\$0\.00/)
  assert.match(html, /部分请求费用未知/)
  assert.match(html, /overflow-wrap:anywhere/)
  assert.match(html, /第 2 页/)
  assert.match(html, /aria-label="下一页" disabled/)
  assert.doesNotMatch(html, /风险分/)
})
