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
// Keep icon stubs on the same Vue runtime as the rendered components.
const icons = Object.fromEntries(
  ['KeyRound', 'Table2', 'Key', 'LinkAlt', 'Openai', 'Xai']
    .map(name => [name, defineComponent({ setup: () => () => h('svg', { 'data-icon': name }) })]),
)

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
      if (name === '@lucide/vue' || name === '@boxicons/vue')
        return icons
      if (name === '@vueuse/core')
        return { createSharedComposable: fn => fn, useNow: () => ref(new Date()) }
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

const component = loadSource(new URL('../src/views/accounts/components/AccountIdentityCell.vue', import.meta.url)).default
const { stablePresetVisualToneClass } = loadSource(new URL('../src/views/accounts/utils/visualTone.ts', import.meta.url))
const account = {
  id: 'acct-state-sample',
  email: 'state-sample@example.invalid',
  provider: 'openai',
  planType: 'team',
  planTypeDisplay: 'Team',
}
function render(fields = {}, props = {}) {
  return renderToString(createSSRApp(component, {
    account: { ...account, ...fields },
    ...props,
  }))
}

function mark(html, name) {
  const element = html.match(new RegExp(`<span\\s[^>]*\\bdata-account-${name}(?=[\\s=>])[^>]*>`))?.[0]
  assert.ok(element, html)
  return element
}

test('Excel selection renders an accessible route icon without changing provider identity', async () => {
  const html = await render({ responsesUpstream: 'excel' })
  assert.match(html, /aria-label="Excel 入口"/)
  assert.match(html, /data-icon="Table2"/)
  assert.match(html, /data-icon="Openai"/)
  assert.match(html, /state-sample@example\.invalid/)
  assert.ok(html.includes(stablePresetVisualToneClass(account.id)))
})

test('retired State fields never alter avatars or enable Excel', async () => {
  const original = await render()
  for (const enabled of [false, true, null]) {
    assert.equal(await render({
      turnStateInjectionEnabled: enabled,
      turnState: { requiredModels: ['model-a'], readyModels: [{ model: 'model-a' }] },
    }), original)
  }
  assert.doesNotMatch(original, /data-account-state|Table2|ring-amber|bg-amber/)
})

test('Excel and 2FA indicators retain fixed sizes and swipe selection handles', async () => {
  for (const size of ['md', 'lg']) {
    const html = await render({ responsesUpstream: 'excel' }, { size, hasTotp: true })
    const totp = mark(html, 'totp-mark')
    assert.match(totp, /absolute -left-1 -top-1/)
    assert.match(totp, /size-4/)
    assert.match(totp, /data-swipe-select-handle/)
    assert.match(html, size === 'lg' ? /size-10 / : /size-9 /)
    assert.match(html, /data-icon="Table2"/)
    assert.match(html, /data-icon="KeyRound"/)
    assert.match(html, /data-swipe-select-ignore/)
  }
})

test('Excel indicator follows the account switch and preserves other identity props', async () => {
  const props = { size: 'lg', hasTotp: true, showPlan: true, titleMode: 'email' }
  const off = await render({ responsesUpstream: 'codex' }, props)
  const on = await render({ responsesUpstream: 'excel' }, props)
  assert.match(on, /data-icon="Table2"/)
  assert.match(on, />Team</)
  assert.match(on, /data-account-totp-mark/)
  assert.equal(await render({ responsesUpstream: 'codex' }, props), off)
  assert.doesNotMatch(off, /data-icon="Table2"|data-account-state/)
})
