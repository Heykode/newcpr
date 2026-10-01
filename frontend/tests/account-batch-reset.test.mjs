/* eslint-disable test/no-import-node-test -- Node regression runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

const tick = () => new Promise(resolve => setImmediate(resolve))
const copy = value => JSON.parse(JSON.stringify(value))
function harness(overrides = {}) {
  const calls = []
  const batch = {
    id: 'batch-fixture',
    createdAt: '2026-10-01T00:00:00Z',
    confirmed: false,
    resetType: null,
    items: [{ accountId: 'account-a', availableCount: 2, credit: { id: 'soon' }, redeemRequestId: 'original', status: 'ready' }],
  }
  const api = {
    getResetInventory: async () => [],
    refreshResetInventory: async (ids) => {
      calls.push(['refresh', copy(ids)])
      return [{ accountId: ids[0], checkedAt: '2026-10-01T00:00:00Z', credits: { availableCount: 2, credits: [] } }]
    },
    previewResetBatch: async (ids) => {
      calls.push(['preview', copy(ids)])
      return copy(batch)
    },
    confirmResetBatch: async (id) => {
      calls.push(['confirm', id])
      return { ...copy(batch), confirmed: true }
    },
    getResetBatches: async () => [],
    retryResetBatch: async (id, accountId) => { calls.push(['retry', id, accountId]) },
    ...overrides,
  }
  const exports = {}
  const text = readFileSync(new URL('../src/views/accounts/composables/useBatchResetCredits.ts', import.meta.url), 'utf8')
  const { outputText } = ts.transpileModule(text, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } })
  let timer
  runInNewContext(outputText, {
    exports,
    AbortController,
    setInterval: fn => (timer = fn),
    clearInterval: () => {},
    require(name) {
      if (name === 'vue')
        return vue
      if (name === '@/api/modules/reset-credits')
        return api
      if (name === '@/utils/async')
        return { errorMessage: e => e.message }
      throw new Error(name)
    },
  })
  const scope = vue.effectScope()
  const selectedIds = vue.ref(new Set(['account-a']))
  let reloads = 0
  const state = scope.run(() => exports.useBatchResetCredits({
    accounts: vue.ref([{ id: 'account-a' }]),
    selectedIds,
    reload: async () => reloads++,
  }))
  return { state, calls, scope, api, selectedIds, batch, timer: () => timer(), reloads: () => reloads }
}

test('batch preview and cancellation do not consume; selected IDs stay frozen', async () => {
  const h = harness()
  try {
    await h.state.prepare()
    assert.deepEqual(h.calls, [['preview', ['account-a']]])
    h.selectedIds.value = new Set(['account-b'])
    h.state.open.value = false
    assert.equal(h.calls.some(c => c[0] === 'confirm'), false)
    h.state.open.value = true
    await h.state.confirm()
    assert.deepEqual(h.calls.at(-1), ['confirm', 'batch-fixture'])
  }
  finally { h.scope.stop() }
})

test('lost confirmation retries the same batch and concurrent double clicks send once', async () => {
  let reject
  const calls = []
  const h = harness({ confirmResetBatch: (id) => {
    calls.push(id)
    return new Promise((_, fail) => {
      reject = fail
    })
  } })
  try {
    await h.state.prepare()
    const first = h.state.confirm()
    await h.state.confirm()
    assert.deepEqual(calls, ['batch-fixture'])
    reject(new Error('network timeout'))
    await first
    assert.equal(h.state.preview.value.id, 'batch-fixture')
    h.api.confirmResetBatch = async (id) => {
      calls.push(id)
      return { ...h.batch, confirmed: true }
    }
    await h.state.confirm()
    assert.deepEqual(calls, ['batch-fixture', 'batch-fixture'])
  }
  finally { h.scope.stop() }
})

test('cache reads never query upstream; refresh failure is not represented as zero', async () => {
  const h = harness()
  try {
    await h.state.refreshSelected()
    assert.equal(h.state.inventory.value['account-a'].credits.availableCount, 2)
    h.api.refreshResetInventory = async () => {
      throw new Error('unavailable')
    }
    await h.state.refreshSelected()
    assert.equal(h.state.inventory.value['account-a'].credits.availableCount, 2)
    assert.equal(h.state.error.value, 'unavailable')
    assert.equal(h.calls.filter(c => c[0] === 'confirm').length, 0)
  }
  finally { h.scope.stop() }
})

test('disposing a page ignores late preview and never creates a consume job', async () => {
  let finish
  const h = harness({ previewResetBatch: () => new Promise((resolve) => {
    finish = resolve
  }) })
  const pending = h.state.prepare()
  h.scope.stop()
  finish(h.batch)
  await pending
  await tick()
  assert.equal(h.state.preview.value, null)
  assert.equal(h.calls.length, 0)
})

test('unknown item retries original batch/account only', async () => {
  const h = harness()
  try {
    h.state.batches.value = [{ ...h.batch, confirmed: true, items: [{ ...h.batch.items[0], status: 'unknown' }] }]
    await tick()
    h.state.batches.value = [{ ...h.batch, confirmed: true, items: [{ ...h.batch.items[0], status: 'unknown' }] }]
    await h.state.retry('account-a')
    assert.deepEqual(h.calls, [['retry', 'batch-fixture', 'account-a']])
  }
  finally { h.scope.stop() }
})

test('a failed refresh cannot reopen an old cancelled consumption preview', async () => {
  const h = harness()
  try {
    await h.state.prepare()
    h.state.open.value = false
    h.selectedIds.value = new Set(['account-b'])
    h.api.refreshResetInventory = async () => {
      throw new Error('unavailable')
    }
    await h.state.refreshSelected()
    assert.equal(h.state.preview.value, null)
    await h.state.confirm()
    assert.equal(h.calls.some(c => c[0] === 'confirm'), false)
  }
  finally { h.scope.stop() }
})

test('single-account reopen restores the server pending card, never substitutes the new earliest card', async () => {
  const exports = {}
  const calls = []
  const oldId = '244e790c-42a3-4ec9-a45d-a32b218bc8ac'
  const snapshot = {
    availableCount: 1,
    credits: [{ id: 'another-card', status: 'available', expiresAt: '2099-01-01T00:00:00Z', resetType: 'codex' }],
    pending: { creditId: 'original-card', redeemRequestId: oldId, retryAfter: '2026-01-01T00:00:00Z' },
  }
  const api = {
    getAccountResetCredits: async () => snapshot,
    consumeAccountResetCredit: async (command) => {
      calls.push(copy(command))
      snapshot.pending = null
      return { code: 'already_redeemed' }
    },
    refreshAccountQuota: async () => ({ account: {} }),
  }
  const dependencies = {
    vue,
    '@/api': api,
    '@/api/request': { ApiError: class extends Error {} },
    '@/components/base/BaseToast': { toast: { error: assert.fail, warning: assert.fail, success: () => {} } },
    '@/utils/async': { errorMessage: e => e.message },
  }
  const text = readFileSync(new URL('../src/views/accounts/composables/useAccountResetCredits.ts', import.meta.url), 'utf8')
  const { outputText } = ts.transpileModule(text, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } })
  runInNewContext(outputText, { exports, AbortController, require: name => dependencies[name] })
  const scope = vue.effectScope()
  try {
    const state = scope.run(() => exports.useAccountResetCredits({ accountId: () => 'acct_pending', onAccountUpdated: () => {} }))
    await state.loadCredits()
    assert.equal(state.ambiguous.value, true)
    state.selectCredit('another-card')
    state.requestConsume()
    await state.confirmConsume()
    assert.deepEqual(calls, [{ accountId: 'acct_pending', creditId: 'original-card', redeemRequestId: oldId }])
    assert.equal(state.ambiguous.value, false)
  }
  finally { scope.stop() }
})
