/* eslint-disable test/no-import-node-test -- Uses the built-in Node runner like the other composable regressions. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

const source = readFileSync(new URL('../src/views/accounts/composables/useAccountConcurrency.ts', import.meta.url), 'utf8')
const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 } }).outputText
const flush = () => new Promise(resolve => setImmediate(resolve))
const snapshot = (page, total, ids) => ({ page, pageSize: 100, total, items: ids.map(leaseId => ({ leaseId, expiresAtMs: 1, detail: null })) })

function harness(t) {
  const requests = []
  const timers = new Map()
  const visibility = vue.ref('visible')
  const account = vue.ref('account-a')
  const open = vue.ref(true)
  const pool = vue.ref('shared')
  const exports = {}
  let nextTimer = 0
  runInNewContext(code, {
    exports,
    AbortController,
    setTimeout: (callback, delay) => {
      const id = ++nextTimer
      timers.set(id, { callback, delay })
      return id
    },
    clearTimeout: id => timers.delete(id),
    require: name => ({
      'vue': vue,
      '@vueuse/core': { useDocumentVisibility: () => visibility },
      '@/api/modules/accounts': { getAccountConcurrency: (params, options) => new Promise((resolve, reject) => requests.push({ params, options, resolve, reject })) },
    })[name],
  })
  const scope = vue.effectScope()
  const query = scope.run(() => exports.useAccountConcurrency(() => account.value, open, pool))
  t.after(() => {
    scope.stop()
    assert.equal(timers.size, 0)
  })
  const tick = () => {
    assert.equal(timers.size, 1)
    const [id, timer] = [...timers.entries()][0]
    assert.equal(timer.delay, 5000)
    timers.delete(id)
    void timer.callback()
  }
  return { requests, timers, visibility, account, open, pool, query, scope, tick }
}

test('concurrency refresh is bounded by its first page, deduplicates leases, and never overlaps', async (t) => {
  const h = harness(t)
  assert.equal(h.requests.length, 1)
  assert.equal(h.requests[0].params.pageSize, 100)
  await h.query.load()
  assert.equal(h.requests.length, 1)
  h.requests[0].resolve(snapshot(1, 201, ['a', 'b']))
  await flush()
  h.requests[1].resolve(snapshot(2, 99999, ['b', 'c']))
  await flush()
  h.requests[2].resolve(snapshot(3, 99999, ['d']))
  await flush()
  assert.equal(h.requests.length, 3)
  assert.equal(h.query.data.value.items.map(item => item.leaseId).join(','), 'a,b,c,d')
  h.tick()
  assert.equal(h.requests.length, 4)
  assert.equal(h.query.data.value.items.length, 4, 'keep the last complete snapshot during refresh')
  h.requests[3].reject(new Error('temporary failure'))
  await flush()
  assert.equal(h.query.data.value.items.length, 4)
  assert.equal(h.query.error.value, '并发详情刷新失败')
})

test('changing account or pool cancels old reads and prevents stale results', async (t) => {
  const h = harness(t)
  const original = h.requests[0]
  h.account.value = 'account-b'
  await vue.nextTick()
  assert.equal(original.options.signal.aborted, true)
  assert.equal(h.requests[1].params.accountId, 'account-b')
  original.resolve(snapshot(1, 1, ['old-account']))
  await flush()
  assert.equal(h.query.data.value, null)
  h.requests[1].resolve(snapshot(1, 1, ['new-account']))
  await flush()
  h.pool.value = 'reserved'
  await vue.nextTick()
  assert.equal(h.query.data.value, null)
  assert.equal(h.requests[2].params.pool, 'reserved')
  h.requests[2].resolve(snapshot(1, 1, ['guardian']))
  await flush()
  assert.equal(h.query.data.value.items[0].leaseId, 'guardian')
})

test('hidden and closed dialogs stop polling and dispose pending requests', async (t) => {
  const h = harness(t)
  h.requests[0].resolve(snapshot(1, 1, ['a']))
  await flush()
  h.visibility.value = 'hidden'
  await vue.nextTick()
  assert.equal(h.timers.size, 0)
  assert.equal(h.requests[0].options.signal.aborted, true)
  h.visibility.value = 'visible'
  await vue.nextTick()
  assert.equal(h.requests.length, 2)
  h.open.value = false
  await vue.nextTick()
  h.requests[1].resolve(snapshot(1, 1, ['stale']))
  await flush()
  assert.equal(h.query.data.value, null)
  assert.equal(h.timers.size, 0)
  h.open.value = true
  await vue.nextTick()
  h.scope.stop()
  assert.equal(h.requests[2].options.signal.aborted, true)
})

test('legacy servers without lease IDs still display every returned row', async (t) => {
  const h = harness(t)
  const first = snapshot(1, 101, ['a'])
  delete first.items[0].leaseId
  h.requests[0].resolve(first)
  await flush()
  const second = snapshot(2, 101, ['b'])
  delete second.items[0].leaseId
  h.requests[1].resolve(second)
  await flush()
  assert.equal(h.query.data.value.items.length, 2)
})
