/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'
import { compileScript, parse } from 'vue/compiler-sfc'

const require = createRequire(import.meta.url)
const filename = new URL('../src/views/settings/components/OutboundUserAgentCard.vue', import.meta.url)
const { descriptor } = parse(readFileSync(filename, 'utf8'), { filename: filename.pathname })
const compiled = compileScript(descriptor, { id: 'qx-user-agent-test' })
const cpr = 'Codex Desktop/0.153.4 (Mac OS 15.7.1; arm64) unknown (Codex Desktop; 26.901.51231)'
const qx = 'codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color'
const customQx = 'codex_cli_rs/0.146.0 (Linux 6.8.0; x86_64) unknown'

function load(text, dependencies) {
  const { outputText } = ts.transpileModule(text, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  const exports = {}
  runInNewContext(outputText, {
    exports,
    require: name => dependencies[name] ?? require(name),
  })
  return exports
}

function view(selection = { mode: 'default' }) {
  const isQx = selection.userAgent?.startsWith('codex') ?? false
  const raw = selection.userAgent ?? null
  return {
    mode: selection.mode,
    customUserAgent: raw,
    defaultUserAgent: cpr,
    effectiveUserAgent: raw ?? cpr,
    effectiveDesktopUserAgent: 'Codex Desktop/26.901.51231 (Mac OS; arm64)',
    coreVersion: isQx ? '0.146.0' : '0.153.4',
    desktopVersion: '26.901.51231',
    osType: isQx ? 'Ubuntu' : 'Mac OS',
    osVersion: isQx ? '22.4.0' : '15.7.1',
    arch: isQx ? 'x86_64' : 'arm64',
    terminal: isQx ? 'xterm-256color' : 'unknown',
    verified: selection.mode === 'default',
    defaultVerifiedAt: '2026-09-13T00:00:00Z',
  }
}

function harness(initial = view(), overrides = {}) {
  let saved = initial
  const requests = []
  const previews = []
  const messages = []
  let poll
  const api = {
    getOutboundUserAgent: async () => saved,
    previewOutboundUserAgent: async (selection) => {
      previews.push(structuredClone(selection))
      return view(selection)
    },
    updateOutboundUserAgent: async (selection) => {
      requests.push(structuredClone(selection))
      if (selection.userAgent === 'invalid')
        throw new Error('UA fields are incoherent')
      saved = view(selection)
      return saved
    },
    ...overrides,
  }
  const component = load(compiled.content, {
    'vue': { ...vue, onMounted: () => {}, onUnmounted: vue.onScopeDispose },
    '@vueuse/core': { useIntervalFn: callback => poll = callback },
    '@/api/modules/outbound-user-agent': api,
    '@/components/base/BaseButton.vue': {},
    '@/components/base/BaseCard.vue': {},
    '@/components/base/BaseCheckbox.vue': {},
    '@/components/base/BaseForm/FormItem.vue': {},
    '@/components/base/BaseTextarea.vue': {},
    '@/components/base/BaseToast': { toast: { success: message => messages.push(message) } },
    '@/utils/async': { errorMessage: error => error.message },
  })
  const scope = vue.effectScope()
  const state = scope.run(() => component.default.setup({}, { expose: () => {} }))
  return { state, requests, previews, messages, poll: () => poll(), stop: () => scope.stop() }
}

test('migrated CLI selections preview save and reload their exact pinned UA', async () => {
  for (const selection of [
    { mode: 'custom', userAgent: customQx },
    { mode: 'custom', userAgent: qx },
  ]) {
    const app = harness(view(selection))
    try {
      await app.state.load(true)
      assert.equal(app.state.tlsProfile, undefined)
      assert.equal(app.state.sessionPolicy, undefined)
      assert.equal(app.state.useDefault.value, false)
      assert.equal(app.state.displayedInput.value, selection.userAgent ?? qx)
      await app.state.check()
      assert.deepEqual(app.previews, [selection])
      assert.equal(app.state.preview.value.verified, false)
      assert.equal(app.requests.length, 0)
      await app.state.save()
      assert.deepEqual(app.requests, [selection])
      app.state.custom.value = cpr
      await app.state.load(true)
      assert.equal(app.state.custom.value, selection.userAgent)
      await app.state.save()
      assert.deepEqual(app.requests, [selection, selection])
    }
    finally {
      app.stop()
    }
  }
})

test('CPR default and explicit custom equal to default remain distinct', async () => {
  for (const selection of [{ mode: 'default' }, { mode: 'custom', userAgent: cpr }]) {
    const app = harness(view(selection))
    try {
      await app.state.load(true)
      assert.equal(app.state.useDefault.value, selection.mode === 'default')
      await app.state.save()
      assert.deepEqual(app.requests, [selection])
    }
    finally {
      app.stop()
    }
  }
})

test('the single input accepts either UA format and retains the draft across checkbox changes', async () => {
  const app = harness(view({ mode: 'custom', userAgent: cpr }))
  try {
    await app.state.load(true)
    app.state.displayedInput.value = qx
    assert.deepEqual(structuredClone(app.state.selection.value), {
      mode: 'custom',
      userAgent: qx,
    })
    app.state.displayedInput.value = customQx
    await app.state.save()
    assert.deepEqual(app.requests, [{
      mode: 'custom',
      userAgent: customQx,
    }])
    app.state.useDefault.value = true
    assert.equal(app.state.displayedInput.value, cpr)
    app.state.useDefault.value = false
    assert.equal(app.state.displayedInput.value, customQx)
    await app.state.save()
    assert.deepEqual(app.requests.at(-1), {
      mode: 'custom',
      userAgent: customQx,
    })
  }
  finally {
    app.stop()
  }
})

test('blank custom UA is rejected instead of switching to default implicitly', async () => {
  const app = harness(view({ mode: 'custom', userAgent: qx }))
  try {
    await app.state.load(true)
    for (const blank of ['', '   ']) {
      app.state.useDefault.value = false
      app.state.displayedInput.value = blank
      await vue.nextTick()
      await app.state.check()
      await app.state.save()
      assert.equal(app.requests.length, 0)
      assert.equal(app.previews.length, 0)
      assert.equal(app.state.useDefault.value, false)
      assert.equal(app.state.error.value, '请填写完整 UA，或勾选使用默认')
    }
    app.state.useDefault.value = false
    app.state.displayedInput.value = '\r\n'
    assert.equal(app.state.selection.value.userAgent, '\r\n', 'unsafe values remain visible to Provider validation')
  }
  finally {
    app.stop()
  }
})

test('rejected QX save retains persisted state and editable input without success feedback', async () => {
  const initial = view()
  const app = harness(initial)
  try {
    await app.state.load(true)
    app.state.useDefault.value = false
    app.state.displayedInput.value = 'invalid'
    await vue.nextTick()
    await app.state.save()
    assert.equal(app.state.settings.value.mode, 'default')
    assert.equal(app.state.displayedInput.value, 'invalid')
    assert.equal(app.state.error.value, 'UA fields are incoherent')
    assert.equal(app.state.saving.value, false)
    assert.equal(app.messages.length, 0)
  }
  finally {
    app.stop()
  }
})

test('a new default release from polling does not reset QX choice or draft', async () => {
  const app = harness(view({ mode: 'custom', userAgent: customQx }), {
    getOutboundUserAgent: async () => ({ ...view({ mode: 'custom', userAgent: customQx }), defaultUserAgent: 'new-default' }),
  })
  try {
    await app.state.load(true)
    app.state.displayedInput.value = `${customQx} (codex_cli_rs; 0.146.0)`
    await app.state.load(false, true)
    assert.equal(app.state.settings.value.defaultUserAgent, 'new-default')
    assert.equal(app.state.displayedInput.value, `${customQx} (codex_cli_rs; 0.146.0)`)
  }
  finally {
    app.stop()
  }
})

test('unmounted QX preview and save cannot publish late results', async () => {
  for (const operation of ['check', 'save']) {
    let resolve
    const pending = new Promise(done => resolve = done)
    const app = harness(view(), {
      [operation === 'check' ? 'previewOutboundUserAgent' : 'updateOutboundUserAgent']: () => pending,
    })
    await app.state.load(true)
    app.state.useDefault.value = false
    app.state.custom.value = qx
    const running = app.state[operation]()
    app.stop()
    resolve(view({ mode: 'custom', userAgent: qx }))
    await running
    assert.equal(app.state.settings.value.mode, 'default')
    assert.equal(app.state.preview.value, null)
    assert.equal(app.messages.length, 0)
  }
})

test('UA API keeps explicit mode, raw input and request cancellation options', async () => {
  const calls = []
  const api = load(readFileSync(new URL('../src/api/modules/outbound-user-agent.ts', import.meta.url), 'utf8'), {
    '../request': config => calls.push(config),
  })
  const options = { silent: true, signal: new AbortController().signal }
  for (const selection of [{ mode: 'default' }, { mode: 'custom', userAgent: customQx }]) {
    await api.previewOutboundUserAgent(selection, options)
    await api.updateOutboundUserAgent(selection, options)
  }
  assert.equal(calls.length, 4)
  for (const [index, call] of calls.entries()) {
    assert.equal(call.method, 'POST')
    assert.equal(call.data.mode, index >= 2 ? 'custom' : 'default')
    assert.equal(call.data.userAgent, index >= 2 ? customQx : undefined)
    assert.equal(call.signal, options.signal)
    assert.equal(call.silent, true)
  }
})

test('all supported UA selections preview save and reload without field loss', async () => {
  for (const userAgent of [null, cpr, qx, customQx]) {
    const selection = userAgent === null ? { mode: 'default' } : { mode: 'custom', userAgent }
    const app = harness(view(selection))
    try {
      await app.state.load(true)
      assert.equal(app.state.useDefault.value, userAgent === null)
      assert.equal(app.state.displayedInput.value, userAgent ?? cpr)
      await app.state.check()
      assert.deepEqual(app.previews, [selection])
      assert.equal(app.requests.length, 0)
      await app.state.save()
      await app.state.load(true)
      assert.deepEqual(app.requests, [selection])
      assert.deepEqual(structuredClone(app.state.selection.value), selection)
    }
    finally {
      app.stop()
    }
  }
})

test('default UA follows polling and remains automatic after saving', async () => {
  const selection = { mode: 'default' }
  let latest = cpr
  const app = harness(view(selection), {
    getOutboundUserAgent: async () => ({ ...view(selection), defaultUserAgent: latest }),
  })
  try {
    await app.state.load(true)
    latest = 'updated-default'
    await app.state.load(false, true)
    assert.equal(app.state.displayedInput.value, latest)
    assert.equal(app.state.useDefault.value, true)
    await app.state.save()
    assert.deepEqual(app.requests, [selection])
  }
  finally {
    app.stop()
  }
})

test('default API does not transmit retired choices or an implicit custom UA', async () => {
  const calls = []
  const api = load(readFileSync(new URL('../src/api/modules/outbound-user-agent.ts', import.meta.url), 'utf8'), {
    '../request': config => calls.push(config),
  })
  const selection = { mode: 'default' }
  const signal = new AbortController().signal
  await api.previewOutboundUserAgent(selection, { signal })
  await api.updateOutboundUserAgent(selection, { signal })
  assert.equal(calls.length, 2)
  for (const call of calls) {
    assert.deepEqual(call.data, selection)
    assert.equal(call.signal, signal)
  }
})
