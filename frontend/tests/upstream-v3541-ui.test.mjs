/* eslint-disable test/no-import-node-test -- project Node regression runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'
import { compileScript, parse } from 'vue/compiler-sfc'
import { renderToString } from 'vue/server-renderer'
import { createTestRequire } from './helpers/i18n.mjs'

const require = createTestRequire(import.meta.url)
const slot = vue.defineComponent({ setup: (_, { slots }) => () => vue.h('div', [slots.default?.(), slots.body?.()]) })

function load(filename, dependencies = {}) {
  const source = readFileSync(filename, 'utf8')
  const script = filename.pathname.endsWith('.vue')
    ? compileScript(parse(source, { filename: filename.pathname }).descriptor, { id: filename.pathname, inlineTemplate: true }).content
    : source
  const exports = {}
  runInNewContext(ts.transpileModule(script, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  }).outputText, {
    exports,
    require(name) {
      if (name in dependencies)
        return dependencies[name]
      if (name === '@/i18n' || (!name.startsWith('.') && !name.startsWith('@/')))
        return require(name)
      if (name.startsWith('@/components/base/'))
        return slot
      const path = /\.(?:vue|ts)$/.test(name) ? name : `${name}.ts`
      return load(name.startsWith('@/') ? new URL(`../src/${path.slice(2)}`, import.meta.url) : new URL(path, filename), dependencies)
    },
  })
  return exports
}

const { summarizeProxyBatch, summarizeProxyTests } = load(new URL('../src/views/proxies/utils/batch.ts', import.meta.url), {
  '@/api/request': { ApiError: class extends Error {} },
})

function item(key, status, location) {
  return {
    key,
    name: key,
    endpoint: 'http://proxy.example:8080',
    status,
    message: `${key} message`,
    record: { id: key, lastTest: { success: status === 'succeeded', latencyMs: 5, location } },
  }
}

test('proxy summaries distinguish unfinished work, attention and location from connectivity', () => {
  const items = ['pending', 'running', 'skipped', 'succeeded', 'failed', 'error', 'conflict', 'busy', 'unknown'].map(status => item(status, status))
  assert.deepEqual({ ...summarizeProxyBatch(items) }, { completed: 6, succeeded: 1, failed: 1, attention: 4, skipped: 1 })
  const tests = [item('ok', 'succeeded', { status: 'detected' }), item('no-location', 'succeeded'), item('lookup', 'succeeded', { status: 'failed' }), item('failed', 'failed')]
  assert.deepEqual({ ...summarizeProxyTests(tests) }, { completed: 4, succeeded: 3, failed: 1, attention: 0, skipped: 0, locationPending: 2 })
  assert.equal(summarizeProxyTests([{ key: 'missing', status: 'succeeded' }]).locationPending, 1)
})

test('proxy results show each saved proxy once and prioritize test errors and location warnings', async () => {
  const component = load(new URL('../src/views/proxies/components/ProxyBatchResults.vue', import.meta.url), {
    '@/api/request': { ApiError: class extends Error {} },
  }).default
  const items = [item('lookup', 'succeeded'), item('connection', 'succeeded'), item('create', 'error')]
  const tests = [item('lookup', 'succeeded', { status: 'failed', message: 'location lookup failed' }), item('connection', 'failed', { status: 'failed', message: 'must not mask connection error' })]
  const html = await renderToString(vue.createSSRApp(component, { items, tests }))
  assert.equal((html.match(/<li /g) ?? []).length, 3)
  assert.match(html, /连接正常/)
  assert.match(html, /location lookup failed/)
  assert.match(html, /connection message/)
  assert.match(html, /create message/)
  assert.doesNotMatch(html, /must not mask connection error/)
  const testsOnly = await renderToString(vue.createSSRApp(component, { tests }))
  assert.doesNotMatch(testsOnly, /已添加/)
  assert.match(testsOnly, /地区待处理/)
})

async function chart(view, loading = false) {
  let option
  const component = load(new URL('../src/views/usage/components/UsageCostCard.vue', import.meta.url), {
    '@/components/charts/BaseChart.vue': vue.defineComponent({ props: ['option'], setup(props) {
      option = props.option
      return () => null
    } }),
    '@/composables/useChartPalette': { useChartPalette: () => ({ palette: vue.ref({ success: '#008800', normal: '#0000cc', textMuted: '#888888' }) }) },
  }).default
  const points = [
    { bucket: 'empty', label: 'Empty', estimatedCost: null, noCacheCost: null, cacheSavings: null, inputTokens: 0, cachedTokenRate: 0, cacheHitRequestRate: null },
    { bucket: 'unknown', label: 'Unknown', estimatedCost: null, noCacheCost: null, cacheSavings: null, inputTokens: 10, cachedTokenRate: 0.5, cacheHitRequestRate: 0.25 },
    { bucket: 'known', label: 'Known', estimatedCost: '1', noCacheCost: '2', cacheSavings: '1', inputTokens: 20, cachedTokenRate: 0.75, cacheHitRequestRate: 1 },
  ]
  const cost = { points, coverage: { known: 1, partial: 0, unknown: 1 }, estimatedCost: '1', cacheSavings: '1', tierPremium: '0', costPerSuccessfulRequest: '0.5' }
  await renderToString(vue.createSSRApp(component, { cost, activity: points.map((point, index) => ({ bucket: point.bucket, totalRequests: index === 0 ? 0 : 1 })), view, loading }))
  return option
}

test('cost series share legend and tooltip semantics while preserving unknown costs', async () => {
  const option = await chart('cost')
  const series = option.series.filter(item => item.name === '费用')
  const primary = series[0]
  assert.deepEqual(Array.from(primary.data), [0, null, 1])
  assert.deepEqual(Array.from(option.legend.data), ['费用', '无缓存估算'])
  const empty = option.tooltip.formatter([{ dataIndex: 0 }])
  const unknown = option.tooltip.formatter([{ dataIndex: 1 }])
  const known = option.tooltip.formatter([{ dataIndex: 2 }])
  assert.match(empty, /\$0/)
  assert.doesNotMatch(empty, /无缓存估算/)
  assert.match(unknown, /—/)
  assert.doesNotMatch(unknown, /无缓存估算/)
  assert.match(known, /无缓存估算/)
  assert.match(known, /\$2/)
  assert.equal(await chart('cost', true), undefined, 'loading hides previous chart data')
})

test('savings and cache views select their own values, legends and axes', async () => {
  const savings = await chart('savings')
  assert.deepEqual(Array.from(savings.legend.data), ['缓存节省'])
  assert.deepEqual(Array.from(savings.series[0].data), [0, null, 1])
  const cache = await chart('cache')
  assert.equal(cache.legend.data.length, 2)
  assert.equal(cache.yAxis.min, 0)
  assert.equal(cache.yAxis.max, 1)
  assert.match(cache.tooltip.formatter([{ dataIndex: 1 }]), /50%/)
  assert.match(cache.tooltip.formatter([{ dataIndex: 1 }]), /25%/)
})
