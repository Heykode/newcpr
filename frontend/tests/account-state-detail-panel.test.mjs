/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import { createSSRApp, defineComponent, h, ref } from 'vue'
import { compileScript, parse } from 'vue/compiler-sfc'
import { renderToString } from 'vue/server-renderer'

const require = createRequire(import.meta.url)
const modules = new Map()
const fixedNow = new Date('2026-09-18T12:00:00Z')
const icons = {
  ShieldCheck: defineComponent({ setup: () => () => h('svg', { 'data-icon': 'ShieldCheck' }) }),
}

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
    require(name) {
      if (name === '@lucide/vue')
        return icons
      if (name === '@vueuse/core')
        return { createSharedComposable: fn => fn, useNow: () => ref(new Date(fixedNow)) }
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

const component = loadSource(new URL('../src/views/accounts/components/AccountTurnStatePanel.vue', import.meta.url)).default

function account(fields = {}) {
  return {
    id: 'acct-state-panel',
    provider: 'openai',
    turnStateInjectionEnabled: true,
    turnState: {
      requiredModels: ['gpt-6-astra', 'gpt-5.6-sol', 'gpt-5.6-terra'],
      readyModels: [
        { model: 'gpt-6-astra', expiresAt: '2026-09-18T12:50:00Z' },
        { model: 'gpt-5.6-sol', expiresAt: '2026-09-18T12:20:00Z' },
      ],
      models: [
        {
          model: 'gpt-6-astra',
          refreshStatus: 'ready',
          active: { chars: 332, expiresAt: '2026-09-18T12:50:00Z' },
          standby: { chars: 332, expiresAt: '2026-09-18T12:55:00Z' },
        },
        {
          model: 'gpt-5.6-sol',
          refreshStatus: 'refreshing',
          active: { chars: 332, expiresAt: '2026-09-18T12:20:00Z' },
          standby: null,
        },
        {
          model: 'gpt-5.6-terra',
          refreshStatus: 'refreshing',
          active: null,
          standby: null,
        },
      ],
    },
    ...fields,
  }
}

function render(fields = {}) {
  return renderToString(createSSRApp(component, { account: account(fields) }))
}

test('expanded account State panel lists model slots, lengths, totals, and countdowns', async () => {
  const html = await render()
  assert.match(html, /data-account-turn-state-panel/)
  assert.match(html, />2\/3<\/strong> 模型/)
  assert.match(html, />3<\/strong> 个 State/)
  assert.match(html, /gpt-6-astra/)
  assert.match(html, /gpt-5\.6-sol/)
  assert.match(html, /gpt-5\.6-terra/)
  assert.equal((html.match(/>332 字符 ·/g) ?? []).length, 3)
  assert.match(html, /332 字符 · 50m/)
  assert.match(html, /332 字符 · 55m/)
  assert.match(html, /补充备用/)
  assert.match(html, /采集中/)
  assert.match(html, /data-icon="ShieldCheck"/)
})

test('State panel explains disabled and unavailable states without inventing captured values', async () => {
  const disabled = await render({ turnStateInjectionEnabled: false })
  assert.match(disabled, /账号级开关已关闭/)
  assert.doesNotMatch(disabled, /个 State/)

  const unavailable = await render({ turnState: null })
  assert.match(unavailable, /全局未启用、账号已暂停或状态暂不可用/)
  assert.doesNotMatch(unavailable, /332 字符/)
})

test('non-OpenAI accounts do not render the managed State panel', async () => {
  assert.doesNotMatch(await render({ provider: 'xai' }), /data-account-turn-state-panel/)
})

test('model list retains every model in a bounded region, keyboard-focusable only when overflowing', async () => {
  for (const count of [2, 3, 4, 10]) {
    const models = Array.from({ length: count }, (_, index) => ({
      model: `model-${index}`,
      refreshStatus: 'refreshing',
      active: null,
      standby: null,
    }))
    const html = await render({
      turnState: { requiredModels: models.map(item => item.model), readyModels: [], models },
    })
    assert.equal((html.match(/data-account-turn-state-model/g) ?? []).length, count)
    assert.match(html, /data-account-turn-state-list[^>]*role="region"[^>]*aria-label="模型 State 状态"/)
    assert.match(html, /max-h-\[10\.5rem\].*overflow-y-auto/)
    assert.match(html, /max-sm:max-h-\[14\.25rem\]/)
    assert.equal(/tabindex="0"/.test(html), count > 3)
    assert.match(html, new RegExp(`>0/${count}</strong> 模型`))
  }
})
