/* eslint-disable test/no-import-node-test */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

function harness() {
  const calls = []
  const modules = new Map()
  const api = Object.fromEntries(['getUsageRecordSummary', 'getUsageHealth', 'getUsagePerformance', 'getUsageCost', 'getUsageRecordInsightsDiagnostics'].map(name =>
    [name, (params, options) => new Promise((resolve, reject) => calls.push({ name, params, ...options, resolve, reject }))]))
  function load(file) {
    if (modules.has(file.href))
      return modules.get(file.href)
    const exports = {}
    modules.set(file.href, exports)
    const { outputText } = ts.transpileModule(readFileSync(file, 'utf8'), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 } })
    runInNewContext(outputText, {
      exports,
      AbortController,
      require(name) {
        if (name === 'vue')
          return vue
        if (name === '@/api')
          return api
        if (name === '@/api/request')
          return { ApiError: class extends Error {} }
        if (name === '@/utils/async')
          return { errorMessage: error => error.message }
        if (name === '@/composables/useRequestState')
          return load(new URL('../src/composables/useRequestState.ts', import.meta.url))
        throw new Error(name)
      },
    })
    return exports
  }
  const scope = vue.effectScope()
  let params = { startTime: '2026-10-30T00:00:00Z', endTime: '2026-10-30T01:00:00Z' }
  const state = scope.run(() => load(new URL('../src/views/usage/composables/useUsageInsights.ts', import.meta.url)).useUsageInsights(() => params))
  return { calls, state, stop: () => scope.stop(), change: (value) => {
    params = value
  } }
}
async function flush() {
  await vue.nextTick()
  await Promise.resolve()
  await vue.nextTick()
}

test('cards publish independently and slow/failed metrics cannot discard successful cards', async () => {
  const h = harness()
  try {
    const pending = h.state.loadUsageAnalytics()
    assert.equal(h.calls.length, 5)
    const summary = h.calls.find(call => call.name === 'getUsageRecordSummary')
    summary.resolve({ totalRequests: '7' })
    await flush()
    assert.equal(h.state.summary.value.data.totalRequests, '7')
    assert.equal(h.state.summary.value.loading, false)
    assert.equal(h.state.health.value.loading, true)
    const performance = h.calls.find(call => call.name === 'getUsagePerformance')
    assert.equal(performance.params.metric, 'latency')
    performance.reject(new Error('synthetic performance failure'))
    await flush()
    assert.equal(h.state.performance.value.error, 'synthetic performance failure')
    assert.equal(h.state.summary.value.data.totalRequests, '7')
    for (const call of h.calls.filter(call => ![summary, performance].includes(call))) call.resolve({})
    await pending
    const count = h.calls.length
    h.state.performanceMetric.value = 'load'
    await flush()
    assert.equal(h.calls.length, count + 1)
    assert.equal(h.calls.at(-1).params.metric, 'load')
    h.calls.at(-1).resolve({ metric: 'load', data: { points: [] } })
    await flush()
    h.state.costMetric.value = 'cache'
    await flush()
    assert.equal(h.calls.at(-1).name, 'getUsageCost')
    assert.equal(h.calls.at(-1).params.metric, 'cache')
    h.calls.at(-1).resolve({})
  }
  finally { h.stop() }
})

test('metric and filter changes cancel old requests and reject late results and errors', async () => {
  const h = harness()
  try {
    h.state.performanceMetric.value = 'firstToken'
    await flush()
    const old = h.calls.at(-1)
    h.state.performanceMetric.value = 'throughput'
    await flush()
    assert.equal(old.signal.aborted, true)
    const current = h.calls.at(-1)
    current.resolve({ metric: 'throughput' })
    await flush()
    old.resolve({ metric: 'firstToken' })
    await flush()
    assert.equal(h.state.performance.value.data.metric, 'throughput')
    h.state.invalidate()
    assert.equal(h.state.performance.value.data, null)
    h.change({ startTime: '2026-10-29T00:00:00Z', endTime: '2026-10-30T00:00:00Z', accountIds: 'fixture-account' })
    const pending = h.state.loadUsageAnalytics()
    const fresh = h.calls.slice(-5)
    assert(fresh.every(call => call.params.accountIds === 'fixture-account'))
    h.stop()
    assert(fresh.every(call => call.signal.aborted))
    for (const call of fresh) call.resolve({ stale: true })
    await pending
    assert.equal(h.state.health.value.data, null)
    assert.equal(h.state.performance.value.data, null)
  }
  finally { h.stop() }
})
