/* eslint-disable test/no-import-node-test -- project Node regression runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'
import { createTestRequire } from './helpers/i18n.mjs'

const baseRequire = createTestRequire(import.meta.url)
function load(path, dependencies = {}) {
  const exports = {}
  const source = readFileSync(new URL(path, import.meta.url), 'utf8')
  runInNewContext(ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS } }).outputText, {
    exports,
    URL,
    TextEncoder,
    AbortController,
    require: name => dependencies[name] ?? (name === '@/api/request' ? load('../src/api/error.ts') : baseRequire(name)),
  })
  return exports
}
const exports = load('../src/views/proxies/utils/batch.ts')
const { parseProxyBatch, proxyBatchInputError } = exports

test('official proxy catalog cancels disposed reads and respects the returned page size', async (t) => {
  const dependencies = { '@/utils/async': { errorMessage: error => error.message } }
  dependencies['./useRequestState'] = load('../src/composables/useRequestState.ts', dependencies)
  dependencies.vue = { ...vue, onMounted: () => {} }
  const calls = []
  const pending = Promise.withResolvers()
  dependencies['@/api'] = { getProxies: async (query, options) => {
    calls.push({ query, options })
    return pending.promise
  } }
  const { useProxyCatalog } = load('../src/composables/useProxyCatalog.ts', dependencies)
  const scope = vue.effectScope()
  t.after(() => scope.stop())
  const catalog = scope.run(() => useProxyCatalog())
  const loading = catalog.loadProxies()
  scope.stop()
  assert.equal(calls[0].options.signal.aborted, true)
  pending.resolve({ items: [{ id: 'stale' }], page: { pageSize: 50, totalPages: 2 } })
  await loading
  assert.equal(calls.length, 1)
  assert.equal(catalog.proxies.value.length, 0)

  dependencies['@/api'] = { getProxies: async (query) => {
    calls.push({ query })
    return { items: [{ id: `page-${query.page}` }], page: { pageSize: 50, totalPages: query.page === 1 ? 2 : 500 } }
  } }
  const nextScope = vue.effectScope()
  t.after(() => nextScope.stop())
  const nextCatalog = nextScope.run(() => load('../src/composables/useProxyCatalog.ts', dependencies).useProxyCatalog())
  await nextCatalog.loadProxies()
  assert.equal(nextCatalog.proxies.value.length, 2)
  assert.equal(calls.at(-1).query.pageSize, 50)
  assert.equal(calls.length, 3)
})

function batchHarness(t, create, testProxy = async () => []) {
  const scope = vue.effectScope()
  t.after(() => scope.stop())
  const dependencies = { '../utils/batch': exports }
  dependencies['./useProxyBatchTest'] = load('../src/views/proxies/composables/useProxyBatchTest.ts', dependencies)
  dependencies['@/api/modules/proxies'] = { createProxyBatch: create }
  const { useProxyBatchCreate } = load('../src/views/proxies/composables/useProxyBatchCreate.ts', dependencies)
  let reloads = 0
  const batch = scope.run(() => useProxyBatchCreate({ test: testProxy, reload: async () => reloads++ }))
  return { batch, scope, reloads: () => reloads }
}

test('batch create skips malformed rows, clears successful credentials and tests saved records', async (t) => {
  const calls = []
  const checked = []
  const { batch } = batchHarness(t, async (rows) => {
    calls.push(rows)
    return rows.map((row, index) => ({ status: 'succeeded', data: { record: { id: `proxy-${index}`, name: row.name, endpoint: 'http://proxy.example:8080', revision: 1 } } }))
  }, async (rows, options, detectLocation) => {
    checked.push({ rows, options, detectLocation })
    return rows.map(record => ({ status: 'succeeded', data: { ...record, lastTest: { success: true, message: 'ok' } } }))
  })
  batch.text.value = 'invalid\nproxy.example:8080:user:secret'
  await batch.submit(true)
  assert.equal(calls[0].length, 1)
  assert.equal(batch.items.value[0].status, 'failed')
  assert.equal(batch.items.value[1].status, 'succeeded')
  assert.equal(batch.items.value[1].input, '')
  assert.equal(batch.items.value[1].proxyUrl, '')
  assert.equal(batch.text.value, '')
  assert.equal(checked[0].detectLocation, true)
  batch.editFailures()
  assert.equal(batch.text.value, 'invalid')
})

test('batch unknown outcome stops later groups and cannot be resubmitted via edit failures', async (t) => {
  const calls = []
  const { batch } = batchHarness(t, async (rows) => {
    calls.push(rows)
    throw new Error('transport lost')
  })
  batch.text.value = Array.from({ length: 12 }, (_, index) => `proxy.example:${8000 + index}`).join('\n')
  await batch.submit(false)
  assert.equal(calls.length, 1)
  assert.equal(calls[0].length, 10)
  assert.equal(batch.items.value.filter(item => item.status === 'unknown').length, 10)
  assert.equal(batch.items.value.filter(item => item.status === 'skipped').length, 2)
  batch.editFailures()
  assert.equal(batch.text.value.split('\n').length, 2)
})

test('explicit stop completes current batch only and disposal clears retained credentials', async (t) => {
  const pending = Promise.withResolvers()
  const { batch, scope } = batchHarness(t, async (rows) => {
    await pending.promise
    return rows.map(row => ({ status: 'failed', httpStatus: 409, code: 40901, message: row.name }))
  })
  batch.text.value = Array.from({ length: 12 }, (_, index) => `proxy.example:${8000 + index}:user:secret`).join('\n')
  const submitting = batch.submit(false)
  batch.stop()
  pending.resolve()
  await submitting
  assert.equal(batch.items.value.filter(item => item.status === 'conflict').length, 10)
  assert.equal(batch.items.value.filter(item => item.status === 'skipped').length, 2)
  scope.stop()
  assert.ok(batch.items.value.every(item => !item.input && !item.proxyUrl))
})

test('official shorthand parser preserves protocols and encodes credentials without exposing them', () => {
  for (const protocol of ['http', 'https', 'socks5', 'socks5h']) {
    const rows = parseProxyBatch('proxy.example:8080\r\n[2001:db8::1]:1080:u:p:a@ss\nhttps://proxy.example:9443', 'Test', protocol)
    assert.equal(rows.length, 3)
    assert.equal(new URL(rows[0].proxyUrl).protocol, `${protocol}:`)
    assert.equal(new URL(rows[1].proxyUrl).password, 'p%3Aa%40ss')
    assert.equal(rows[1].endpoint.includes('@'), false)
    assert.equal(rows[2].proxyUrl, 'https://proxy.example:9443')
    assert.equal(rows[2].name, 'Test-003')
    assert.ok(rows.every(row => !row.error))
  }
})

test('official parser reports duplicates and malformed shorthand with safe fixed messages', () => {
  const rows = parseProxyBatch('\nproxy.example:8080\rproxy.example:8080\nsecret-invalid-input', '', 'http')
  assert.equal(rows[0].line, 2)
  assert.ok(rows[1].error)
  assert.ok(rows[2].error)
  assert.equal(rows[2].error.includes('secret-invalid-input'), false)
  assert.equal(proxyBatchInputError('a'.repeat(512 * 1024), 100), '')
  assert.ok(proxyBatchInputError('a'.repeat(512 * 1024 + 1), 1))
  assert.ok(proxyBatchInputError('', 101))
})
