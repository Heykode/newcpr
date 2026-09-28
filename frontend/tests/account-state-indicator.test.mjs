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

test('Excel selection renders an accessible green avatar frame without changing provider identity', async () => {
  const html = await render({ responsesUpstream: 'excel' })
  const avatar = mark(html, 'avatar')
  assert.match(avatar, /data-account-excel-status="enabled"/)
  assert.match(avatar, /border-cp-success /)
  assert.match(avatar, /aria-label="[^"]*Excel 模式已开启"/)
  assert.match(avatar, /title="[^"]*Excel 模式已开启"/)
  assert.match(avatar, /role="img"/)
  assert.doesNotMatch(html, /data-icon="Table2"|aria-label="Excel 入口"/)
  assert.match(html, /data-icon="Openai"/)
  assert.match(html, /state-sample@example\.invalid/)
  assert.ok(!avatar.includes(stablePresetVisualToneClass(account.id)))
})

test('Excel 403 historical warning survives scheduling and route changes', async () => {
  for (const enabled of [false, true]) {
    for (const responsesUpstream of ['excel', 'codex']) {
      const html = await render({
        enabled,
        responsesUpstream,
        excel403WarningAt: '2026-09-27T00:00:00Z',
        excelAutoDisabledAt: null,
      })
      const avatar = mark(html, 'avatar')
      assert.match(avatar, /data-account-excel-status="warning"/)
      assert.match(avatar, /border-cp-warning /)
      assert.match(avatar, /bg-cp-warning-container/)
      assert.match(avatar, /BPS 403疑似被封excel/)
      assert.match(avatar, /历史标记/)
      assert.match(avatar, /不代表当前调度状态/)
      assert.match(avatar, responsesUpstream === 'excel' ? /Excel 模式已开启/ : /Excel 模式未开启/)
      assert.doesNotMatch(avatar, /border-cp-success/)
      assert.doesNotMatch(html, />\s*BPS 403疑似被封excel\s*</)
      assert.doesNotMatch(html, /Excel 403 自动暂停调度|data-icon="Table2"/)
    }
  }
})

test('Excel warning supports legacy pause data without marking clean accounts', async () => {
  const legacy = await render({ excelAutoDisabledAt: '2026-09-27T00:00:00Z' })
  assert.match(mark(legacy, 'avatar'), /data-account-excel-status="warning"/)
  for (const fields of [{}, { excel403WarningAt: null, excelAutoDisabledAt: '2026-09-27T00:00:00Z' }, { excelModeDisabledAt: '2026-09-27T00:00:00Z' }]) {
    const html = await render(fields)
    assert.doesNotMatch(html, /BPS 403疑似被封excel/)
    assert.match(mark(html, 'avatar'), /data-account-excel-status="default"/)
    assert.ok(mark(html, 'avatar').includes(stablePresetVisualToneClass(account.id)))
  }
  assert.match(await render({ excelModeDisabledAt: '2026-09-27T00:00:00Z' }), />\s*Excel 403 自动关闭\s*</)
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
    const avatar = mark(html, 'avatar')
    const totp = mark(html, 'totp-mark')
    assert.match(totp, /absolute -left-1 -top-1/)
    assert.match(totp, /size-4/)
    assert.match(totp, /data-swipe-select-handle/)
    assert.match(avatar, size === 'lg' ? /size-10 / : /size-9 /)
    assert.match(avatar, /border-2/)
    assert.match(avatar, /data-swipe-select-handle/)
    assert.match(avatar, /data-account-excel-status="enabled"/)
    assert.doesNotMatch(html, /data-icon="Table2"/)
    assert.match(html, /data-icon="KeyRound"/)
    assert.match(html, /data-swipe-select-ignore/)
  }
})

test('Excel indicator follows the account switch and preserves other identity props', async () => {
  const props = { size: 'lg', hasTotp: true, showPlan: true, titleMode: 'email' }
  const off = await render({ responsesUpstream: 'codex' }, props)
  const on = await render({ responsesUpstream: 'excel' }, props)
  assert.match(mark(on, 'avatar'), /data-account-excel-status="enabled"/)
  assert.match(on, />Team</)
  assert.match(on, /data-account-totp-mark/)
  assert.equal(await render({ responsesUpstream: 'codex' }, props), off)
  assert.match(mark(off, 'avatar'), /data-account-excel-status="default"/)
  assert.match(mark(off, 'avatar'), /border-2[^"]*border-transparent/)
  assert.ok(mark(off, 'avatar').includes(stablePresetVisualToneClass(account.id)))
  assert.doesNotMatch(off, /data-icon="Table2"|data-account-state/)
})
