/* eslint-disable test/no-import-node-test -- uses the project's Node test runner. */
import assert from 'node:assert/strict'
import { webcrypto } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'
import { parse } from 'vue/compiler-sfc'

function evaluate(source, dependencies = {}, globals = {}) {
  const exports = {}
  const { outputText } = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 } })
  runInNewContext(outputText, { exports, AbortController, crypto: webcrypto, ...globals, require: name => dependencies[name] ?? {} })
  return exports
}
const { currentTotp } = evaluate(readFileSync(new URL('../src/views/relogin/totp.ts', import.meta.url), 'utf8'))
const { recentOperationAt, recentOperation } = evaluate(readFileSync(new URL('../src/views/relogin/maintenance.ts', import.meta.url), 'utf8'))

test('recent operation time follows the newer login or maintenance for both layouts and filters', () => {
  const row = { lastAttemptAt: '2026-01-02T00:00:00Z', maintenance: { action: 'devices', checkedAt: '2026-01-01T00:00:00Z' } }
  assert.equal(recentOperationAt(row), row.lastAttemptAt)
  assert.equal(recentOperation(row), '获取凭据')
  row.maintenance.checkedAt = '2026-01-03T00:00:00Z'
  assert.equal(recentOperationAt(row), row.maintenance.checkedAt)
  assert.equal(recentOperation(row), '设备与会话')
  assert.equal(recentOperationAt({}), undefined)
})

test('TOTP matches RFC 6238 SHA-1 vectors and changes on the 30-second boundary', async () => {
  const secret = 'GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ'
  for (const [seconds, expected] of [[59, '287082'], [1111111109, '081804'], [1111111111, '050471'], [1234567890, '005924'], [2000000000, '279037'], [20000000000, '353130']])
    assert.equal(await currentTotp(secret, seconds * 1000), expected)
  assert.equal(await currentTotp(secret, 59000), await currentTotp(secret, 59999))
  assert.notEqual(await currentTotp(secret, 59999), await currentTotp(secret, 60000))
  await assert.rejects(currentTotp('invalid!'), /密钥格式无效/)
})

async function flush() {
  for (let i = 0; i < 12; i++) await Promise.resolve()
  await vue.nextTick()
}
function modal(t, filename, api, names) {
  const props = vue.reactive({ entry: { id: 'demo', revision: 7, email: 'demo@example.invalid' } })
  const open = vue.ref(false)
  const events = []
  const timers = new Map()
  const scope = vue.effectScope()
  const { descriptor } = parse(readFileSync(new URL(`../src/views/relogin/components/${filename}`, import.meta.url), 'utf8'))
  const state = scope.run(() => evaluate(`${descriptor.scriptSetup.content}\nexport { ${names.join(', ')} }`, {
    'vue': { ...vue, onBeforeUnmount: vue.onScopeDispose },
    '@/api/modules/relogin': api,
    '@/utils/async': { errorMessage: error => error.message },
    '@/composables/useCopyText': { useCopyText: () => () => {} },
    '../totp': { currentTotp },
  }, {
    defineProps: () => props,
    defineEmits: () => event => events.push(event),
    defineModel: () => open,
    setInterval(callback) {
      const id = timers.size + 1
      timers.set(id, callback)
      return id
    },
    clearInterval(id) { timers.delete(id) },
  }))
  t.after(() => {
    open.value = false
    scope.stop()
  })
  open.value = true
  return { state, props, open, events, timers }
}

test('maintenance confirmation preserves the revision the user saw', async (t) => {
  const writes = []
  const h = modal(t, 'ReloginMaintenanceModal.vue', {
    maintainRelogin: async (data) => {
      writes.push(data)
      throw new Error('资料已变化')
    },
    getRelogin: () => { throw new Error('must not replace the confirmed revision') },
  }, ['ask', 'pending', 'run', 'failure', 'tab'])
  h.state.tab.value = 'security'
  h.state.ask('logout_all')
  h.props.entry = { ...h.props.entry, revision: 8 }
  await flush()
  assert.equal(h.state.tab.value, 'security')
  await h.state.run('logout_all', undefined, undefined, true, h.state.pending.value.revision)
  assert.equal(writes.length, 1)
  assert.equal(writes[0].revision, 7)
  assert.equal(h.state.failure.value, '资料已变化')
})

test('maintenance shortcut tab follows the initial-tab prop even when visibility updates first', async (t) => {
  const h = modal(t, 'ReloginMaintenanceModal.vue', {}, ['tab'])
  h.props.initialTab = 'devices'
  await flush()
  assert.equal(h.state.tab.value, 'devices')
  h.state.tab.value = 'workspaces'
  h.props.entry = { ...h.props.entry, revision: 8 }
  await flush()
  assert.equal(h.state.tab.value, 'workspaces')
})

test('closing maintenance prevents a delayed result reopening or replacing the next account', async (t) => {
  const reply = Promise.withResolvers()
  let reads = 0
  const h = modal(t, 'ReloginMaintenanceModal.vue', {
    maintainRelogin: () => reply.promise,
    getRelogin: async () => {
      reads++
      return { items: [] }
    },
  }, ['run', 'result', 'current'])
  const running = h.state.run('devices')
  h.open.value = false
  h.props.entry = { id: 'other', revision: 2 }
  h.open.value = true
  reply.resolve({ state: 'succeeded', devices: [{ id: 'old-device' }] })
  await running
  assert.equal(h.state.result.value, undefined)
  assert.equal(h.state.current.value.id, 'other')
  assert.equal(reads, 0)
  assert.ok(h.events.includes('changed'))
})

test('opening credentials closes maintenance instead of stacking dialogs', (t) => {
  const h = modal(t, 'ReloginMaintenanceModal.vue', {}, ['showCredentials'])
  h.state.showCredentials()
  assert.equal(h.open.value, false)
  assert.ok(h.events.includes('credentials'))
})

test('history does not describe device and security checks as successful logins', async (t) => {
  const h = modal(t, 'ReloginHistoryModal.vue', {
    getReloginHistory: async () => ({ items: [], nextCursor: null }),
  }, ['attemptResultLabel'])
  await flush()
  assert.equal(h.state.attemptResultLabel({ operation: 'devices', result: 'succeeded' }), '已完成')
  assert.equal(h.state.attemptResultLabel({ operation: 'disable_totp', result: 'failed' }), '失败')
  assert.equal(h.state.attemptResultLabel({ operation: null, result: 'succeeded' }), '登录成功')
})

test('material saves the revision of the sensitive snapshot and discards late reads after closure', async (t) => {
  const writes = []
  const late = Promise.withResolvers()
  let reads = 0
  const h = modal(t, 'ReloginCredentialsModal.vue', {
    exportRelogin: async () => ({ files: [] }),
    getReloginMaterial: async () => {
      reads++
      return reads === 1 ? { revision: 19, password: 'fixture-password', mfaSecret: 'GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ', history: [] } : late.promise
    },
    updateReloginMaterial: async data => writes.push(data),
  }, ['format', 'saveMaterial', 'material', 'password', 'secret'])
  h.state.format.value = 'edit'
  await flush()
  h.props.entry.revision = 20
  await h.state.saveMaterial()
  assert.equal(writes[0].revision, 19)
  h.state.format.value = 'history'
  h.open.value = false
  late.resolve({ revision: 21, password: 'late-secret', mfaSecret: 'late-key', history: [] })
  await flush()
  assert.equal(h.state.material.value, null)
  assert.equal(h.state.password.value, '')
  assert.equal(h.state.secret.value, '')
  assert.equal(h.timers.size, 0)
})
