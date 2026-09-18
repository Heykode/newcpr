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
const modules = new Map()
// Keep icon stubs on the same Vue runtime as the rendered components.
const icons = Object.fromEntries(
  ['KeyRound', 'ShieldCheck', 'Key', 'LinkAlt', 'Openai', 'Xai']
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

test('account State opt-in renders gold avatar and an accessible badge without changing provider', async () => {
  const html = await render({ turnStateInjectionEnabled: true })
  assert.match(mark(html, 'state-avatar'), /bg-amber-100.*ring-2 ring-inset ring-amber-500/)
  assert.match(mark(html, 'state-avatar'), /\[html\[data-theme=dark\]_&amp;\]:ring-amber-400/)
  assert.match(mark(html, 'state-mark'), /role="img"/)
  assert.match(mark(html, 'state-mark'), /title="[^"]*State[^"]*"/)
  assert.match(mark(html, 'state-mark'), /aria-label="[^"]*State[^"]*"/)
  assert.match(html, /data-icon="ShieldCheck"/)
  assert.match(html, /data-icon="Openai"/)
  assert.match(html, /state-sample@example\.invalid/)
  assert.ok(!html.includes(stablePresetVisualToneClass(account.id)))
})

test('disabled or absent State field keeps the original avatar and no State badge', async () => {
  const original = await render()
  assert.equal(await render({ turnStateInjectionEnabled: false }), original)
  assert.equal(await render({ turnStateInjectionEnabled: null }), original)
  assert.ok(original.includes(stablePresetVisualToneClass(account.id)))
  assert.doesNotMatch(original, /data-account-state|ShieldCheck|ring-amber|bg-amber/)
})

test('State badge is OpenAI-only and partial account props remain compatible', async () => {
  for (const provider of ['xai', undefined]) {
    const html = await render({ provider, turnStateInjectionEnabled: true })
    assert.doesNotMatch(html, /data-account-state|ShieldCheck|ring-amber|bg-amber/)
    assert.ok(html.includes(stablePresetVisualToneClass(account.id)))
  }
})

test('State and 2FA badges retain separate corners, fixed sizes and swipe selection handles', async () => {
  for (const size of ['md', 'lg']) {
    const html = await render({ turnStateInjectionEnabled: true }, { size, hasTotp: true })
    const state = mark(html, 'state-mark')
    const totp = mark(html, 'totp-mark')
    assert.match(state, /absolute -bottom-1 -right-1/)
    assert.match(totp, /absolute -left-1 -top-1/)
    for (const badge of [state, totp]) {
      assert.match(badge, /size-4/)
      assert.match(badge, /data-swipe-select-handle/)
    }
    assert.match(mark(html, 'state-avatar'), size === 'lg' ? /size-10 / : /size-9 /)
    assert.match(mark(html, 'state-avatar'), /data-swipe-select-handle/)
    assert.match(html, /data-icon="KeyRound"/)
    assert.match(html, /data-swipe-select-ignore/)
  }
})

test('State styling is derived anew from the account switch and preserves other identity props', async () => {
  const props = { size: 'lg', hasTotp: true, showPlan: true, titleMode: 'email' }
  const off = await render({ turnStateInjectionEnabled: false }, props)
  const on = await render({ turnStateInjectionEnabled: true }, props)
  assert.match(on, /data-account-state-mark/)
  assert.match(on, />Team</)
  assert.match(on, /data-account-totp-mark/)
  assert.equal(await render({ turnStateInjectionEnabled: false }, props), off)
  assert.doesNotMatch(off, /data-account-state/)
})
