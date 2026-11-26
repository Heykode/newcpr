/* eslint-disable test/no-import-node-test -- Isolated lifecycle regression runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'
import { createTestRequire } from './helpers/i18n.mjs'

const require = createTestRequire(import.meta.url)
function harness(kind) {
  const calls = []
  const notifications = []
  const mounts = []
  const modules = new Map()
  const api = Object.fromEntries(['getPricing', 'previewPricingSync', 'updatePricing', 'syncPricing', 'previewPrivacyPolicy'].map(name => [name, (...args) => new Promise((resolve, reject) => calls.push({ name, args, resolve, reject }))]))
  function load(path) {
    if (modules.has(path))
      return modules.get(path)
    const exports = {}
    modules.set(path, exports)
    const { outputText } = ts.transpileModule(readFileSync(new URL(`../src/${path}`, import.meta.url), 'utf8'), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 } })
    runInNewContext(outputText, {
      exports,
      AbortController,
      require(name) {
        if (name === 'vue')
          return { ...vue, onMounted: callback => mounts.push(callback) }
        if (name === '@/api')
          return api
        if (name === '@/api/request')
          return load('api/error.ts')
        if (name === '@/utils/async')
          return { errorMessage: error => error.message }
        if (name === '@/components/base/BaseToast')
          return { toast: { success: message => notifications.push(message), error: message => notifications.push(message) } }
        if (name.startsWith('@/composables/') || name === '@/providers/catalog' || name === '@/utils/providers')
          return load(`${name.slice(2)}.ts`)
        if (name === './model')
          return load('views/settings/components/pricing/model.ts')
        return require(name)
      },
    })
    return exports
  }
  const scope = vue.effectScope()
  const state = scope.run(() => kind === 'pricing'
    ? load('views/settings/components/pricing/usePricing.ts').usePricing()
    : load('views/settings/components/privacy/usePrivacyPreview.ts').usePrivacyPreview())
  return { state, calls, notifications, mount: () => mounts[0](), stop: () => scope.stop() }
}
const catalog = marker => ({ defaults: {}, overrides: {}, synced: {}, supportedBands: { openai: ['standard'], antigravity: ['standard'] }, syncedAt: null, syncedAtDisplay: marker })
async function flush() {
  await Promise.resolve()
  await vue.nextTick()
  await Promise.resolve()
}

test('privacy preview exposes current errors, cancels changed samples and ignores late responses after close', async () => {
  const h = harness('privacy')
  try {
    const old = h.state.run({ enabled: true, rules: [] }, { body: {} })
    const first = h.calls[0]
    h.state.invalidate()
    assert(first.args[1].signal.aborted)
    const pending = h.state.run({ enabled: true, rules: [] }, { body: { newer: true } })
    h.calls[1].reject(new Error('synthetic validation error'))
    await pending
    assert.equal(h.state.error.value, 'synthetic validation error')
    first.resolve({ body: { stale: true } })
    await old
    assert.equal(h.state.result.value, undefined)
    h.state.invalidate()
    assert.equal(h.state.error.value, '')
    const after = h.state.run({ enabled: true, rules: [] }, { body: {} })
    h.stop()
    assert(h.calls[2].args[1].signal.aborted)
    h.calls[2].resolve({ body: { stale: true } })
    await after
    assert.equal(h.state.result.value, undefined)
  }
  finally { h.stop() }
})

test('pricing scope disposal aborts reads and provider changes fence synchronization previews', async () => {
  const h = harness('pricing')
  try {
    const loaded = h.mount()
    h.calls[0].resolve(catalog('initial'))
    await loaded
    assert.equal(h.state.provider.value, 'openai')
    const old = h.state.startSync()
    const request = h.calls[1]
    h.state.provider.value = 'antigravity'
    assert(request.args[0].signal.aborted)
    request.resolve({ prices: {}, skipped: [] })
    await old
    assert.equal(h.state.syncOpen.value, false)
    const current = h.state.startSync()
    h.stop()
    assert(h.calls[2].args[0].signal.aborted)
    h.calls[2].resolve({ prices: {}, skipped: [] })
    await current
    assert.equal(h.state.syncOpen.value, false)
    assert.equal(h.notifications.length, 0)
  }
  finally { h.stop() }
})

test('pricing writes freeze targets and cannot reopen or refresh a disposed page', async () => {
  const h = harness('pricing')
  try {
    const loaded = h.mount()
    h.calls[0].resolve(catalog('initial'))
    await loaded
    const models = ['fixture-model']
    const change = vue.reactive({ action: 'multiplier', multiplierBps: 12500 })
    const save = h.state.save(models, change)
    assert.equal(h.calls[1].name, 'updatePricing')
    models.push('unselected-model')
    change.multiplierBps = 90000
    h.state.provider.value = 'antigravity'
    assert.equal(h.calls[1].args[0].provider, 'openai')
    assert.equal(h.calls[1].args[0].models.length, 1)
    assert.equal(h.calls[1].args[0].change.multiplierBps, 12500)
    assert.equal(await h.state.save(['duplicate'], change), false)
    h.stop()
    h.calls[1].resolve({ saved: true })
    assert.equal(await save, false)
    await flush()
    assert.equal(h.calls.length, 2)
    assert.equal(h.notifications.length, 0)
  }
  finally { h.stop() }
})
