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
const idSet = load('../src/composables/useIdSet.ts', { vue })

test('probe verdicts reuse exact quality-page wording and independent semantic colors', () => {
  const { qualityVerdictLabel: label, qualityStatusColor: color } = load('../src/components/quality-ops/presentation.ts')
  assert.equal(label('correct', 'state_probe'), '智商正常')
  assert.equal(color('correct'), 'text-cp-success')
  assert.equal(label('incorrect', 'state_probe'), '智商异常')
  assert.equal(color('incorrect'), 'text-cp-error')
  assert.equal(label('unknown', 'state_probe'), '无法判断')
  assert.equal(color('unknown'), 'text-cp-text-secondary')
  assert.equal(color('overloaded'), 'text-cp-warning')
  assert.equal(label(null, 'state_probe'), '等待首次检测')
  assert.equal(label('correct', 'answer'), '通过')
})

function quality(t, api, reload = async () => {}) {
  const messages = []
  const { useAccountQuality } = load('../src/views/accounts/composables/useAccountQuality.ts', {
    vue,
    '@/api/modules/quality-ops': api,
    '@/composables/useIdSet': idSet,
    '@/utils/async': { errorMessage: e => e.message },
    '@/components/base/BaseToast': { toast: { error: s => messages.push(s), success: s => messages.push(s), warning: s => messages.push(s) } },
  })
  const scope = vue.effectScope()
  t.after(() => scope.stop())
  return { state: scope.run(() => useAccountQuality(reload)), messages, stop: () => scope.stop() }
}

test('quality toggle rereads version, sends enabled-only patch and coalesces synchronous clicks', async (t) => {
  const read = Promise.withResolvers()
  const writes = []
  let reads = 0
  const { state } = quality(t, {
    getQualityMonitoring: () => {
      reads++
      return read.promise
    },
    setQualityEnabled: async data => writes.push(data),
  })
  const account = { id: 'a', qualityMonitoring: { enabled: true, revision: 1 } }
  const first = state.toggle(account)
  const second = state.toggle(account)
  assert.equal(reads, 1)
  read.resolve({ a: { ruleId: 'r', revision: 9, enabled: true } })
  await Promise.all([first, second])
  assert.deepEqual(structuredClone(writes), [{ id: 'r', revision: 9, enabled: false }])
  assert.equal(state.togglingQualityIds.value.size, 0)
})

test('quality toggle preserves captured intent, setup cancellation and failed-write reconciliation', async (t) => {
  let monitor = { a: { ruleId: 'r', revision: 4, enabled: false } }
  let writes = 0
  let reloads = 0
  const { state, messages } = quality(t, {
    getQualityMonitoring: async () => monitor,
    setQualityEnabled: async () => {
      writes++
      throw new Error('version conflict')
    },
  }, async () => { reloads++ })
  await state.toggle({ id: 'a', qualityMonitoring: { enabled: true } })
  assert.equal(writes, 0, 'another pause must not be inverted')
  monitor = {}
  await state.toggle({ id: 'a' })
  assert.equal(state.setupOpen.value, true)
  state.setupOpen.value = false
  assert.equal(writes, 0)
  monitor = { a: { ruleId: 'r', revision: 5, enabled: false } }
  await state.toggle({ id: 'a', qualityMonitoring: { enabled: false } })
  assert.equal(writes, 1)
  assert.equal(reloads, 3)
  assert.ok(messages.includes('version conflict'))
  assert.equal(monitor.a.enabled, false, 'no optimistic success')
})

test('disposing during quality read aborts it and prevents a late mutation', async (t) => {
  const read = Promise.withResolvers()
  let signal
  let writes = 0
  const { state, stop } = quality(t, {
    getQualityMonitoring: (_ids, options) => {
      signal = options.signal
      return read.promise
    },
    setQualityEnabled: async () => { writes++ },
  })
  const pending = state.toggle({ id: 'a', qualityMonitoring: { enabled: true } })
  stop()
  assert.equal(signal.aborted, true)
  read.resolve({ a: { ruleId: 'r', revision: 1, enabled: true } })
  await pending
  assert.equal(writes, 0)
})

test('batch quality freezes cross-page selection, deduplicates and only patches differing configured rules', async (t) => {
  const read = Promise.withResolvers()
  const reads = []
  const writes = []
  let reloads = 0
  const { state, messages } = quality(t, {
    getQualityMonitoring: (ids) => {
      reads.push([...ids])
      return read.promise
    },
    setQualityEnabled: async data => writes.push(data),
  }, async () => { reloads++ })
  const ids = ['on-page', 'off-page', 'missing', 'on-page']
  const first = state.setBatchEnabled(ids, true)
  ids.splice(0, ids.length, 'unselected')
  await state.setBatchEnabled(['unselected'], false)
  await state.toggle({ id: 'on-page', qualityMonitoring: { enabled: true } })
  assert.equal(reads.length, 1, 'batch and single clicks cannot interleave')
  assert.equal(state.batchBusy.value, true)
  read.resolve({
    'on-page': { ruleId: 'rule-a', revision: 8, enabled: false },
    'off-page': { ruleId: 'rule-b', revision: 9, enabled: true },
    'unselected': { ruleId: 'rule-c', revision: 2, enabled: false },
  })
  await first
  assert.deepEqual(reads[0], ['on-page', 'off-page', 'missing'])
  assert.deepEqual(structuredClone(writes), [{ id: 'rule-a', revision: 8, enabled: true }])
  assert.ok(messages.some(text => text.includes('已开启 2 个') && text.includes('1 个未配置') && text.includes('监测模板')))
  assert.equal(reloads, 1)
  assert.equal(state.batchBusy.value, false)
})

test('batch quality pauses mixed states, keeps partial failures and rereads before a manual retry', async (t) => {
  const monitoring = {
    a: { ruleId: 'r-a', revision: 2, enabled: true },
    b: { ruleId: 'r-b', revision: 3, enabled: true },
    c: { ruleId: 'r-c', revision: 4, enabled: false },
  }
  const writes = []
  let rejectB = true
  let reads = 0
  const { state, messages } = quality(t, {
    getQualityMonitoring: async () => {
      reads++
      return structuredClone(monitoring)
    },
    setQualityEnabled: async (data) => {
      writes.push(data)
      if (data.id === 'r-b' && rejectB)
        throw new Error('规则已更新')
      const current = Object.values(monitoring).find(rule => rule.ruleId === data.id)
      assert.equal(data.revision, current.revision)
      current.enabled = data.enabled
      current.revision++
    },
  })
  const ids = new Set(['a', 'b', 'c', 'missing'])
  await state.setBatchEnabled(ids, false)
  assert.deepEqual(structuredClone(writes), [
    { id: 'r-a', revision: 2, enabled: false },
    { id: 'r-b', revision: 3, enabled: false },
  ])
  assert.ok(messages.some(text => text.includes('已关闭 2 个') && text.includes('1 个未配置') && text.includes('1 个未完成：规则已更新')))
  assert.equal(monitoring.b.enabled, true)
  assert.equal(ids.size, 4, 'selection remains usable for the opposite action')
  rejectB = false
  monitoring.b.revision = 10
  await state.setBatchEnabled(ids, false)
  assert.equal(reads, 2)
  assert.deepEqual(structuredClone(writes.slice(2)), [{ id: 'r-b', revision: 10, enabled: false }])
})

test('batch quality does not write on monitoring failure and distinguishes a refresh failure from a saved switch', async (t) => {
  let failRead = true
  let writes = 0
  const { state, messages } = quality(t, {
    getQualityMonitoring: async () => {
      if (failRead)
        throw new Error('监测状态读取失败')
      return { a: { ruleId: 'r', revision: 2, enabled: false } }
    },
    setQualityEnabled: async () => { writes++ },
  }, async () => false)
  await state.setBatchEnabled(['a'], true)
  assert.equal(writes, 0)
  assert.ok(messages.includes('监测状态读取失败'))
  assert.equal(state.batchBusy.value, false)
  failRead = false
  messages.length = 0
  await state.setBatchEnabled(['a'], true)
  assert.equal(writes, 1)
  assert.ok(messages.includes('质量检测已开启 1 个'))
  assert.ok(messages.includes('账号列表刷新失败，请手动刷新核对状态'))
  assert.ok(!messages.some(text => text.includes('未完成')))
})

test('batch quality limits concurrent writes and stops unsent work on disposal', async (t) => {
  const gates = Array.from({ length: 7 }, () => Promise.withResolvers())
  const monitoring = Object.fromEntries(gates.map((_, i) => [String(i), { ruleId: `r-${i}`, revision: 1, enabled: false }]))
  const started = Promise.withResolvers()
  let writes = 0
  let active = 0
  let peak = 0
  let reloads = 0
  const { state, messages, stop } = quality(t, {
    getQualityMonitoring: async () => monitoring,
    setQualityEnabled: async () => {
      const gate = gates[writes++]
      peak = Math.max(peak, ++active)
      if (writes === 4)
        started.resolve()
      await gate.promise
      active--
    },
  }, async () => { reloads++ })
  const pending = state.setBatchEnabled(Object.keys(monitoring), true)
  await started.promise
  assert.equal(peak, 4)
  assert.equal(writes, 4)
  stop()
  gates.forEach(gate => gate.resolve())
  await pending
  assert.equal(writes, 4, 'unsent mutations stop, accepted writes finish')
  assert.equal(reloads, 0)
  assert.deepEqual(messages, [])
  assert.equal(state.batchBusy.value, false)
})

test('batch quality rejects empty/oversized selections and waits for single-account setup or switching', async (t) => {
  let reads = 0
  const read = Promise.withResolvers()
  const { state } = quality(t, {
    getQualityMonitoring: async () => {
      reads++
      return read.promise
    },
    setQualityEnabled: async () => assert.fail('no write expected'),
  })
  await state.setBatchEnabled([], true)
  await state.setBatchEnabled(Array.from({ length: 1001 }, (_, i) => String(i)), true)
  state.setupOpen.value = true
  await state.setBatchEnabled(['a'], true)
  state.setupOpen.value = false
  assert.equal(reads, 0)
  const single = state.toggle({ id: 'a' })
  await state.setBatchEnabled(['a'], true)
  assert.equal(reads, 1)
  read.resolve({})
  await single
})

test('disposing during the batch monitoring read aborts it without later writes', async (t) => {
  const read = Promise.withResolvers()
  let signal
  const { state, stop, messages } = quality(t, {
    getQualityMonitoring: (_ids, options) => {
      signal = options.signal
      return read.promise
    },
    setQualityEnabled: async () => assert.fail('no write after disposal'),
  })
  const pending = state.setBatchEnabled(['a'], true)
  stop()
  assert.equal(signal.aborted, true)
  read.resolve({ a: { ruleId: 'r', revision: 1, enabled: false } })
  await pending
  assert.deepEqual(messages, [])
})

function deletion(t, deleteAccounts) {
  const errors = []
  const toast = { error: s => errors.push(s), success() {} }
  const asyncAction = load('../src/composables/useAsyncAction.ts', {
    vue,
    '@/api/request': load('../src/api/error.ts'),
    '@/components/base/BaseToast': { toast },
    '@/utils/async': load('../src/utils/async.ts'),
  })
  const { useAccountMutations } = load('../src/views/accounts/composables/useAccountMutations.ts', {
    vue,
    '@/api': { deleteAccounts },
    '@/components/base/BaseToast': { toast },
    '@/composables/useIdSet': idSet,
    '@/composables/useAsyncAction': asyncAction,
    '@/composables/useDownload': { useDownload: () => ({ downloadJson() {} }) },
    '@/utils/async': { errorMessage: e => e.message },
    './useAccountOnboarding': { useAccountOnboarding: () => ({}) },
  })
  const accounts = vue.ref([{ id: 'a', provider: 'openai' }, { id: 'b', provider: 'xai' }])
  const selectedIds = vue.ref(new Set(['a', 'b']))
  const scope = vue.effectScope()
  t.after(() => scope.stop())
  return { state: scope.run(() => useAccountMutations({ accounts, selectedIds, reload: async () => {}, replaceAccount() {}, onImportTaskCreated() {} })), accounts, selectedIds, errors }
}

test('delete checkbox resets per opening and freezes batch choice across providers', async (t) => {
  const first = Promise.withResolvers()
  const requests = []
  const { state, accounts, selectedIds } = deletion(t, async (data) => {
    requests.push(structuredClone(data))
    if (requests.length === 1)
      await first.promise
    return {}
  })
  state.requestDeleteAccount(accounts.value[0])
  assert.equal(state.deleteRelogin.value, false)
  state.deleteRelogin.value = true
  state.showSingleDeleteModal.value = false
  assert.equal(requests.length, 0)
  state.requestDeleteAccount(accounts.value[1])
  assert.equal(state.deleteRelogin.value, false)
  state.showDeleteModal.value = true
  state.batchDeleteRelogin.value = true
  const pending = state.handleBatchDelete()
  await state.handleBatchDelete()
  assert.equal(requests.length, 1)
  state.batchDeleteRelogin.value = false
  first.resolve()
  await pending
  assert.equal(requests.length, 2)
  assert.ok(requests.every(item => item.deleteRelogin === true))
  assert.equal(selectedIds.value.size, 0)
  state.showDeleteModal.value = true
  assert.equal(state.batchDeleteRelogin.value, false)
})

test('partial deletion keeps only failed accounts selected and reports exact success', async (t) => {
  const { state, selectedIds, errors } = deletion(t, async (data) => {
    if (data.provider === 'xai')
      throw new Error('fixture failure')
    return {}
  })
  state.showDeleteModal.value = true
  await state.handleBatchDelete()
  assert.deepEqual([...selectedIds.value], ['b'])
  assert.equal(state.showDeleteModal.value, true)
  assert.ok(errors.some(text => text.includes('已删除 1 个账号')))
})
