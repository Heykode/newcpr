/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import { createSSRApp, defineComponent, h } from 'vue'
import { compileScript, parse } from 'vue/compiler-sfc'
import { renderToString } from 'vue/server-renderer'

const require = createRequire(import.meta.url)
const filename = new URL('../src/views/accounts/components/AccountHealthTimeline.vue', import.meta.url)
const { descriptor } = parse(readFileSync(filename, 'utf8'), { filename: filename.pathname })
const compiled = compileScript(descriptor, { id: 'account-health-test', inlineTemplate: true })
const { outputText } = ts.transpileModule(compiled.content, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
})
const exports = {}
const popover = defineComponent({
  props: ['modelValue', 'hoverDelay', 'placement', 'trigger'],
  setup: (props, { slots }) => () => h('div', {
    'data-hover-delay': props.hoverDelay,
    'data-trigger': props.trigger,
  }, [
    slots.trigger?.({ open: props.modelValue }),
    slots.default?.({ open: true }),
  ]),
})
runInNewContext(outputText, {
  exports,
  require: name => name === '@/components/base/BasePopover.vue' ? popover : require(name),
}, { filename: filename.pathname })

function bucket(index, counts = {}) {
  return {
    key: `bucket-${index}`,
    startAt: new Date(Date.UTC(2026, 8, 12, 0, index * 5)).toISOString(),
    requestCount: 0,
    successCount: 0,
    errorCount: 0,
    nonCompletionCount: 0,
    inFlightCount: 0,
    ...counts,
  }
}

function renderBuckets(buckets) {
  return renderToString(createSSRApp(exports.default, { buckets }))
}

function buttonsFromHtml(html) {
  return [...html.matchAll(/<button\b[^>]*>/g)].map(match => match[0])
}

async function buttons(buckets) {
  return buttonsFromHtml(await renderBuckets(buckets))
}

function popoverDetails(html) {
  return [...html.matchAll(/<dl\b[^>]*>(.*?)<\/dl>/gs)].map(([, content]) => Object.fromEntries(
    [...content.matchAll(/<dt\b[^>]*>([^<]*)<\/dt>\s*<dd\b[^>]*>([^<]*)<\/dd>/g)]
      .map(([, label, value]) => [label.trim(), value.trim()]),
  ))
}

test('health buckets use the threshold palette and retain empty and pending states', async () => {
  const html = await renderBuckets([
    bucket(0),
    bucket(1, { requestCount: 10, successCount: 10 }),
    bucket(2, { requestCount: 10, successCount: 9, errorCount: 1 }),
    bucket(3, { requestCount: 10, errorCount: 10 }),
    bucket(4, { inFlightCount: 2 }),
    bucket(5, { requestCount: 1 }),
  ])
  const rendered = buttonsFromHtml(html)
  assert.equal(rendered.length, 6)
  for (const [index, color, label] of [
    [0, 'bg-slate-400', '无请求'],
    [1, 'bg-emerald-500', '正常'],
    [2, 'bg-amber-400', '部分失败'],
    [3, 'bg-red-500', '成功率偏低'],
    [4, 'bg-slate-400', '等待结果'],
    [5, 'bg-slate-400', '等待结果'],
  ]) {
    assert.ok(rendered[index].includes(color), rendered[index])
    assert.ok(rendered[index].includes(label), rendered[index])
    assert.ok(!rendered[index].includes('bg-blue-500'), 'pending is not maintenance')
  }
  const details = popoverDetails(html)
  for (const index of [0, 4, 5]) {
    assert.equal(details[index].成功率, '暂无样本')
    assert.equal(details[index].已结束请求数, '0')
    assert.match(rendered[index], /成功率 暂无样本，已结束请求 0 次/)
  }
  assert.ok(rendered.every(button => button.includes('type="button"') && button.includes('aria-expanded="false"')))
  assert.equal([...html.matchAll(/data-hover-delay="100"/g)].length, 6)
  assert.equal([...html.matchAll(/data-trigger="hover-click"/g)].length, 6)
})

for (const [successCount, color, label, rate] of [
  [0, 'bg-red-500', '成功率偏低', '0%'],
  [7999, 'bg-red-500', '成功率偏低', '79.99%'],
  [8000, 'bg-amber-400', '部分失败', '80%'],
  [8001, 'bg-amber-400', '部分失败', '80.01%'],
  [9499, 'bg-amber-400', '部分失败', '94.99%'],
  [9500, 'bg-emerald-500', '正常', '95%'],
  [9501, 'bg-emerald-500', '正常', '95.01%'],
  [10000, 'bg-emerald-500', '正常', '100%'],
]) {
  test(`health bucket at ${rate} uses ${color} with its terminal sample size`, async () => {
    const html = await renderBuckets([bucket(0, {
      requestCount: 10000,
      successCount,
      errorCount: 10000 - successCount,
    })])
    const [button] = buttonsFromHtml(html)
    assert.ok(button.includes(color), button)
    assert.ok(button.includes(label), button)
    assert.ok(button.includes(`成功率 ${rate}，已结束请求 10000 次`), button)
    const [details] = popoverDetails(html)
    assert.equal(details.成功率, rate)
    assert.equal(details.已结束请求数, '10000')
  })
}

test('health rate text does not round a below-threshold value up across the threshold', async () => {
  const html = await renderBuckets([
    bucket(0, { requestCount: 100000, successCount: 79999, errorCount: 20001 }),
    bucket(1, { requestCount: 100000, successCount: 94999, errorCount: 5001 }),
  ])
  const rendered = buttonsFromHtml(html)
  assert.ok(rendered[0].includes('bg-red-500'))
  assert.ok(rendered[1].includes('bg-amber-400'))
  assert.deepEqual(popoverDetails(html).map(details => details.成功率), ['79.99%', '94.99%'])
})

test('health buckets preserve completed outcomes during new requests and never invent extra history', async () => {
  assert.equal((await buttons([])).length, 0)
  assert.equal((await buttons([bucket(0)])).length, 1)
  const rendered = await buttons([
    bucket(0, { requestCount: 1, errorCount: 1 }),
    ...Array.from({ length: 6 }, (_, index) => bucket(index + 1, {
      requestCount: 2,
      successCount: 2,
      inFlightCount: 1,
    })),
  ])
  assert.equal(rendered.length, 6)
  assert.ok(rendered.every(button => button.includes('bg-emerald-500')))
  assert.ok(rendered.every(button => !button.includes('bg-red-500')))
  assert.ok(rendered.every(button => button.includes('成功率 100%，已结束请求 2 次')))
})

test('health outcomes are mutually exclusive and non-completions remain in the success denominator', async () => {
  const html = await renderBuckets([
    bucket(0, { requestCount: 100, successCount: 80, errorCount: 0, nonCompletionCount: 20, inFlightCount: 50 }),
    bucket(1, { requestCount: 100, successCount: 95, errorCount: 0, nonCompletionCount: 5, inFlightCount: 50 }),
    bucket(2, { requestCount: 6, successCount: 1, errorCount: 2, nonCompletionCount: 3 }),
    bucket(3, { requestCount: 100, successCount: 80, errorCount: 20 }),
    bucket(4),
  ])
  const rendered = buttonsFromHtml(html)
  for (const [index, color] of [[0, 'bg-amber-400'], [1, 'bg-emerald-500'], [2, 'bg-red-500']])
    assert.ok(rendered[index].includes(color), rendered[index])
  assert.match(rendered[0], /进行中 50 次/)
  assert.deepEqual(popoverDetails(html), [
    { 成功率: '80%', 已结束请求数: '100', 成功: '80', 失败: '0', 未完成: '20', 进行中: '50' },
    { 成功率: '95%', 已结束请求数: '100', 成功: '95', 失败: '0', 未完成: '5', 进行中: '50' },
    { 成功率: '16.66%', 已结束请求数: '6', 成功: '1', 失败: '2', 未完成: '3', 进行中: '0' },
    { 成功率: '80%', 已结束请求数: '100', 成功: '80', 失败: '20', 未完成: '0', 进行中: '0' },
    { 成功率: '暂无样本', 已结束请求数: '0', 成功: '0', 失败: '0', 未完成: '0', 进行中: '0' },
  ])
  assert.match(html, /取消或未正常完成的请求/)
})

test('only non-completions is zero percent, not empty or successful', async () => {
  const html = await renderBuckets([bucket(0, { requestCount: 3, nonCompletionCount: 3 })])
  assert.match(buttonsFromHtml(html)[0], /bg-red-500/)
  assert.deepEqual(popoverDetails(html)[0], {
    成功率: '0%',
    已结束请求数: '3',
    成功: '0',
    失败: '0',
    未完成: '3',
    进行中: '0',
  })
  assert.doesNotMatch(html, />\s*样本数\s*</)
})
