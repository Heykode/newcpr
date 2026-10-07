/* eslint-disable test/no-import-node-test -- Node's built-in regression runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'
import { compileScript, parse } from 'vue/compiler-sfc'
import { renderToString } from 'vue/server-renderer'
import { accounts } from './fixtures/relogin-count-data.mjs'

const components = '../src/views/accounts/components/'
const stub = { __esModule: true, default: { render: () => null } }
function component(path, dependencies = {}) {
  const filename = new URL(components + path, import.meta.url)
  const { descriptor } = parse(readFileSync(filename, 'utf8'), { filename: filename.pathname })
  const script = compileScript(descriptor, { id: path, inlineTemplate: true })
  const { outputText } = ts.transpileModule(script.content, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  })
  const exports = {}
  runInNewContext(outputText, {
    exports,
    require: name => name === 'vue' ? vue : dependencies[name] ?? stub,
  })
  return { __esModule: true, default: exports.default }
}
const credits = component('AccountQuotaPanel/Credits.vue')
const panel = component('AccountQuotaPanel/index.vue', {
  './Credits.vue': credits,
  '../../constants': { groupedAccountQuotaWindows: () => [], orderedPanelQuotaWindows: () => [] },
})
const summary = component('AccountQuotaSummaryCell/index.vue', {
  '../AccountQuotaPanel/Credits.vue': credits,
  '../../constants': { groupedAccountQuotaWindows: () => [], visibleSummaryQuotaWindows: () => [] },
  '@/composables/useUiClock': { useUiClock: () => vue.ref(new Date()) },
  './forecast': { weeklyForecastPresentation: () => ({ amount: '-', title: '' }) },
  './presenter': { recentlyUsedQuotaEntry: () => null },
})

test('Codex balance is visible in both account list and normal quota panel without any reset card', async () => {
  const account = structuredClone(accounts[0])
  for (const [value, expected] of [
    [{ hasCredits: true, unlimited: false, balance: '123.45' }, '123.45'],
    [{ hasCredits: false, unlimited: false, balance: '0' }, '0'],
    [{ hasCredits: false, unlimited: false, balance: null }, '暂无可用点数'],
    [{ hasCredits: true, unlimited: false, balance: null }, '未提供余额'],
    [{ hasCredits: true, unlimited: true, balance: '0' }, '无限'],
    [{ hasCredits: true, unlimited: false, balance: '12345678901234567890.125' }, '12,345,678,901,234,567,890.125'],
    [null, '未提供余额'],
    [undefined, '未提供余额'],
  ]) {
    account.quota.credits = value
    for (const view of [panel, summary]) {
      const html = await renderToString(vue.createSSRApp(view.default, { account, refreshing: false }))
      assert.ok(html.includes('Codex 点数'))
      assert.ok(html.includes(`>${expected}</strong>`), expected)
    }
  }
})

test('other providers do not display a Codex credit balance', async () => {
  const account = { ...structuredClone(accounts[0]), provider: 'xai' }
  account.quota.credits = { hasCredits: true, unlimited: false, balance: '123.45' }
  for (const view of [panel, summary]) {
    const html = await renderToString(vue.createSSRApp(view.default, { account, refreshing: false }))
    assert.ok(!html.includes('Codex 点数'))
    assert.ok(!html.includes('123.45'))
  }
})
