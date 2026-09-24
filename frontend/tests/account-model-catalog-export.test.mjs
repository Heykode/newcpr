/* eslint-disable test/no-import-node-test -- uses the project's Node test runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

const require = createRequire(import.meta.url)

function load(path, dependencies = {}) {
  const exports = {}
  const { outputText } = ts.transpileModule(readFileSync(new URL(path, import.meta.url), 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024, esModuleInterop: true },
  })
  runInNewContext(outputText, { exports, AbortController, require: name => dependencies[name] ?? require(name) })
  return exports
}

function mountExporter(t, getAccountModelCatalog, downloadJson) {
  const files = []
  const errors = []
  const successes = []
  const { useAccountMutations } = load('../src/views/accounts/composables/useAccountMutations.ts', {
    vue,
    '@/api': { getAccountModelCatalog },
    '@/components/base/BaseToast': { toast: { error: text => errors.push(text), success: text => successes.push(text) } },
    '@/composables/useIdSet': load('../src/composables/useIdSet.ts', { vue }),
    '@/composables/useAsyncAction': { useAsyncAction: () => ({ loading: vue.ref(false) }) },
    '@/composables/useDownload': { useDownload: () => ({
      downloadJson: downloadJson ?? (async (payload, name) => files.push({ payload, name })),
    }) },
    '@/utils/async': { errorMessage: error => error.message },
    './useAccountOnboarding': { useAccountOnboarding: () => ({}) },
  })
  const scope = vue.effectScope()
  t.after(() => scope.stop())
  const state = scope.run(() => useAccountMutations({
    accounts: vue.ref([]),
    selectedIds: vue.ref(new Set(['unrelated-selected-account'])),
    reload: assert.fail,
    replaceAccount: assert.fail,
    onImportTaskCreated: assert.fail,
  }))
  return { state, files, errors, successes, stop: () => scope.stop() }
}

test('catalog API uses a single account GET and forwards cancellation options', async () => {
  let sent
  const api = load('../src/api/modules/accounts.ts', {
    '../request': async config => sent = config,
  })
  const controller = new AbortController()
  await api.getAccountModelCatalog({ accountId: 'acct_selected' }, { silent: true, signal: controller.signal })
  assert.equal(sent.method, 'GET')
  assert.equal(sent.url, '/api/admin/accounts/models/catalog')
  assert.deepEqual(structuredClone(sent.params), { accountId: 'acct_selected' })
  assert.equal(sent.signal, controller.signal)
  assert.equal(sent.data, undefined)
})

test('native export downloads only the catalog and coalesces duplicate row actions', async (t) => {
  let resolve
  const requests = []
  const exported = mountExporter(t, (payload) => {
    requests.push(structuredClone(payload))
    return new Promise(done => resolve = done)
  })
  const account = { id: 'acct_selected', provider: 'openai', enabled: false }
  const first = exported.state.handleExportModelCatalog(account)
  const second = exported.state.handleExportModelCatalog(account)
  assert.deepEqual(requests, [{ accountId: 'acct_selected' }])
  assert.equal(exported.state.exportingModelCatalogIds.value.has(account.id), true)
  const catalog = { models: [{ slug: 'native-model', future: { value: null } }] }
  resolve({ modelCount: 1, observedAt: '2026-09-24T00:00:00Z', catalog })
  await Promise.all([first, second])
  assert.equal(exported.files.length, 1)
  assert.deepEqual(exported.files[0].payload, catalog)
  assert.equal(exported.files[0].name, 'cpr-model-catalog-acct_selected.json')
  assert.equal(exported.state.exportingModelCatalogIds.value.size, 0)
  assert.equal(exported.successes.length, 1)
})

test('export failure never downloads a fallback and disposal cancels pending reads', async (t) => {
  let signal
  let resolve
  let fail = true
  const exported = mountExporter(t, async (_, options) => {
    signal = options.signal
    if (fail)
      throw new Error('catalog unavailable')
    return new Promise(done => resolve = done)
  })
  const account = { id: 'acct_selected', provider: 'openai' }
  await exported.state.handleExportModelCatalog(account)
  assert.deepEqual(exported.files, [])
  assert.deepEqual(exported.errors, ['catalog unavailable'])
  assert.equal(exported.state.exportingModelCatalogIds.value.size, 0)
  fail = false
  const pending = exported.state.handleExportModelCatalog(account)
  exported.stop()
  assert.equal(signal.aborted, true)
  resolve({ modelCount: 1, catalog: { models: [{ slug: 'stale' }] } })
  await pending
  assert.deepEqual(exported.files, [])
  assert.deepEqual(exported.successes, [])
})

test('different accounts can fetch concurrently but cannot overwrite the shared download', async (t) => {
  const requests = []
  const started = []
  const files = []
  const firstDownload = Promise.withResolvers()
  const downloadStarted = Promise.withResolvers()
  let currentPayload
  const exported = mountExporter(t, async ({ accountId }) => {
    requests.push(accountId)
    return { modelCount: 1, catalog: { models: [{ slug: accountId }] } }
  }, async (payload, name) => {
    currentPayload = payload
    started.push(name)
    if (started.length === 1) {
      downloadStarted.resolve()
      await firstDownload.promise
    }
    files.push({ payload: currentPayload, name })
  })
  const first = exported.state.handleExportModelCatalog({ id: 'acct_first', provider: 'openai' })
  const second = exported.state.handleExportModelCatalog({ id: 'acct_second', provider: 'openai' })
  await downloadStarted.promise
  assert.deepEqual(requests, ['acct_first', 'acct_second'])
  assert.deepEqual(started, ['cpr-model-catalog-acct_first.json'])
  firstDownload.resolve()
  await Promise.all([first, second])
  assert.deepEqual(files, [
    { payload: { models: [{ slug: 'acct_first' }] }, name: 'cpr-model-catalog-acct_first.json' },
    { payload: { models: [{ slug: 'acct_second' }] }, name: 'cpr-model-catalog-acct_second.json' },
  ])
})

test('queued catalog downloads are skipped after disposal', async (t) => {
  const started = []
  const firstDownload = Promise.withResolvers()
  const downloadStarted = Promise.withResolvers()
  const exported = mountExporter(t, async ({ accountId }) => ({
    modelCount: 1,
    catalog: { models: [{ slug: accountId }] },
  }), async (_, name) => {
    started.push(name)
    downloadStarted.resolve()
    await firstDownload.promise
  })
  const first = exported.state.handleExportModelCatalog({ id: 'acct_first', provider: 'openai' })
  const second = exported.state.handleExportModelCatalog({ id: 'acct_second', provider: 'openai' })
  await downloadStarted.promise
  exported.stop()
  firstDownload.resolve()
  await Promise.all([first, second])
  assert.deepEqual(started, ['cpr-model-catalog-acct_first.json'])
  assert.deepEqual(exported.successes, [])
  assert.deepEqual(exported.errors, [])
})

test('a failed download does not poison the next account export', async (t) => {
  const started = []
  const exported = mountExporter(t, async () => ({ modelCount: 1, catalog: { models: [] } }), async (_, name) => {
    started.push(name)
    if (started.length === 1)
      throw new Error('download failed')
  })
  await exported.state.handleExportModelCatalog({ id: 'acct_first', provider: 'openai' })
  await exported.state.handleExportModelCatalog({ id: 'acct_second', provider: 'openai' })
  assert.deepEqual(started, ['cpr-model-catalog-acct_first.json', 'cpr-model-catalog-acct_second.json'])
  assert.deepEqual(exported.errors, ['download failed'])
  assert.equal(exported.successes.length, 1)
})
