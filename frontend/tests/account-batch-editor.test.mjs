/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import axios from 'axios'
import ts from 'typescript'
import * as vue from 'vue'

const require = createRequire(import.meta.url)
const updateFields = ['updateEnabled', 'updateExcelEnabled', 'updateExcelModels', 'updateConcurrencyLimit', 'updateWeight', 'updateGroups', 'updateProxy', 'updateCustomName', 'updateModelAccess']

function loadModule(filename, dependencies = {}) {
  const exports = {}
  const { outputText } = ts.transpileModule(readFileSync(filename, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, {
    exports,
    require: name => dependencies[name] ?? require(name),
    TextEncoder,
  }, { filename: String(filename) })
  return exports
}

const schedulingForm = loadModule(new URL('../src/views/accounts/utils/schedulingForm.ts', import.meta.url))
const asyncUtils = loadModule(new URL('../src/utils/async.ts', import.meta.url))
const apiErrors = loadModule(new URL('../src/api/error.ts', import.meta.url))

function account(id, overrides = {}) {
  return {
    id,
    provider: 'openai',
    authenticationKind: 'oauth',
    excelModels: ['gpt-5.6-sol'],
    excelModelsFollowGlobal: false,
    enabled: true,
    concurrencyLimit: 8,
    weight: 17,
    groups: [{ id: 'group-existing', name: 'Existing group' }],
    ...overrides,
  }
}

function mountEditor(t, options = {}) {
  const accounts = vue.ref(options.accounts ?? [account('account-a')])
  const selectedIds = vue.ref(new Set(options.selectedIds ?? accounts.value.map(row => row.id)))
  const requests = []
  const messages = { warning: [], error: [], success: [] }
  const reloads = { accounts: 0, groups: 0 }
  const reloadOptions = []
  const toast = Object.fromEntries(Object.keys(messages).map(kind => [kind, message => messages[kind].push(message)]))
  const request = options.adapter && loadModule(new URL('../src/api/request.ts', import.meta.url), {
    './constants': { API_BASE_URL: '', API_TIMEOUT_MS: 1000 },
    './error': apiErrors,
    '@/components/base/BaseToast': { toast },
  })
  const accountsApi = request && loadModule(new URL('../src/api/modules/accounts.ts', import.meta.url), {
    '../request': config => request.default({ ...config, adapter: options.adapter }),
  })
  const asyncAction = loadModule(new URL('../src/composables/useAsyncAction.ts', import.meta.url), {
    vue,
    '@/api/request': apiErrors,
    '@/components/base/BaseToast': { toast },
    '@/utils/async': asyncUtils,
  })
  const module = loadModule(new URL('../src/views/accounts/composables/useAccountBatchEditor.ts', import.meta.url), {
    vue,
    '@/api': {
      batchUpdateAccounts: async (payload) => {
        // Preserve explicit undefined keys while normalizing VM object prototypes.
        requests.push(structuredClone(payload))
        return accountsApi ? accountsApi.batchUpdateAccounts(payload) : options.batchUpdateAccounts?.(payload)
      },
    },
    '@/components/base/BaseToast': { toast },
    '@/composables/useAsyncAction': asyncAction,
    '../utils/schedulingForm': schedulingForm,
    '../utils/modelAccess': loadModule(new URL('../src/views/accounts/utils/modelAccess.ts', import.meta.url)),
    '@/utils/account-name': loadModule(new URL('../src/utils/account-name.ts', import.meta.url)),
  })
  const scope = vue.effectScope()
  t.after(() => scope.stop())
  const state = scope.run(() => module.useAccountBatchEditor({
    accounts,
    selectedIds,
    reloadAccounts: async (requestOptions) => {
      reloads.accounts += 1
      reloadOptions.push(requestOptions)
      await options.reloadAccounts?.()
    },
    reloadGroups: async () => {
      reloads.groups += 1
      await options.reloadGroups?.()
    },
  }))
  return { state, accounts, selectedIds, requests, messages, reloads, reloadOptions }
}

function assertNoUpdates(state) {
  for (const field of updateFields)
    assert.equal(state[field].value, false, `${field} must require a fresh opt-in`)
  assert.equal(state.hasUpdates.value, false)
  assert.equal(state.updateExcelCacheCreationAsInput.value, false)
}

test('Excel cache billing is separately opted in and does not implicitly switch routes', async (t) => {
  const editor = mountEditor(t, { accounts: [account('account-a', { responsesUpstream: 'excel' })] })
  const { state } = editor
  state.open()
  assert.equal(state.excelCacheCreationAsInput.value, false)
  state.excelCacheCreationAsInput.value = true
  assert.equal(state.hasUpdates.value, false)
  state.updateExcelCacheCreationAsInput.value = true
  await state.save()
  assert.deepEqual(editor.requests, [{ accountIds: ['account-a'], excelCacheCreationAsInput: true }])
  await vue.nextTick()
  assertNoUpdates(state)
  editor.selectedIds.value = new Set(['account-a'])
  state.open()
  state.updateExcelCacheCreationAsInput.value = true
  state.excelCacheCreationAsInput.value = false
  await state.save()
  assert.deepEqual(editor.requests[1], { accountIds: ['account-a'], excelCacheCreationAsInput: false })
})

test('a committed batch save completes before slow or failed list reloads', async (t) => {
  let release
  const pending = new Promise(resolve => release = resolve)
  const editor = mountEditor(t, {
    reloadAccounts: () => pending,
    reloadGroups: async () => { throw new Error('list refresh failed after commit') },
  })
  editor.state.open()
  editor.state.updateWeight.value = true
  const saving = editor.state.save()
  try {
    await new Promise(resolve => setImmediate(resolve))
    assert.equal(editor.state.saving.value, false)
    assert.equal(editor.state.showBatchEditModal.value, false)
    assert.equal(editor.messages.success.length, 1)
    assert.deepEqual(editor.messages.error, [])
    assert.equal(editor.selectedIds.value.size, 0)
  }
  finally {
    release()
    await saving
  }
})

function assertNoRequest(editor) {
  assert.deepEqual(editor.requests, [])
  assert.deepEqual(editor.reloads, { accounts: 0, groups: 0 })
  assert.deepEqual(editor.messages.error, [])
  assert.deepEqual(editor.messages.success, [])
  assert.equal(editor.state.saving.value, false)
}

test('Excel model lists require opt-in and can be cleared without changing the route', async (t) => {
  const editor = mountEditor(t)
  const { state } = editor
  state.open()
  state.excelModels.value = 'invalid/model'
  state.updateWeight.value = true
  await state.save()
  assert.deepEqual(editor.requests[0], { accountIds: ['account-a'], weight: 17 })
  await vue.nextTick()
  editor.selectedIds.value = new Set(['account-a'])
  state.open()
  assertNoUpdates(state)
  state.updateExcelModels.value = true
  state.excelModels.value = 'gpt-5.6-sol, gpt-6-astra, gpt-5.6-sol'
  await state.save()
  assert.deepEqual(editor.requests[1], {
    accountIds: ['account-a'],
    excelModels: ['gpt-5.6-sol', 'gpt-6-astra'],
    excelModelsFollowGlobal: false,
  })
  await vue.nextTick()
  editor.selectedIds.value = new Set(['account-a'])
  state.open()
  state.updateExcelModels.value = true
  state.excelModels.value = ''
  await state.save()
  assert.deepEqual(editor.requests[2], { accountIds: ['account-a'], excelModels: [], excelModelsFollowGlobal: false })
})

test('model restrictions are opt-in, validate only when checked and clear explicitly', async (t) => {
  const editor = mountEditor(t, {
    accounts: [account('account-a', { modelAccess: { mode: 'denylist', models: ['model-a'] } })],
  })
  const { state } = editor
  state.open()
  assert.equal(state.catalogAccountId.value, 'account-a')
  state.modelAccess.value = { mode: 'allowlist', models: [] }
  state.updateModelAccess.value = true
  await state.save()
  assertNoRequest(editor)
  assert.equal(editor.messages.warning.length, 1)
  state.updateModelAccess.value = false
  state.updateWeight.value = true
  await state.save()
  assert.deepEqual(editor.requests, [{ accountIds: ['account-a'], weight: 17 }])
  await vue.nextTick()
  editor.selectedIds.value = new Set(['account-a'])
  state.open()
  assertNoUpdates(state)
  state.updateModelAccess.value = true
  state.modelAccess.value = { mode: 'denylist', models: ['model-b'] }
  await state.save()
  assert.deepEqual(editor.requests[1], {
    accountIds: ['account-a'],
    modelAccess: { mode: 'denylist', models: ['model-b'] },
  })
  await vue.nextTick()
  assert.equal(state.modelAccess.value, undefined)
  assert.equal(state.catalogAccountId.value, undefined)
})

test('custom names require opt-in, support clear, and ignore unchecked invalid text', async (t) => {
  for (const [value, expected] of [['  Batch name  ', 'Batch name'], ['', null]]) {
    const editor = mountEditor(t, { accounts: [account('account-a', { customName: 'Original' })] })
    editor.state.open()
    assert.equal(editor.state.customName.value, 'Original')
    editor.state.customName.value = value
    assert.equal(editor.state.hasUpdates.value, false)
    editor.state.updateCustomName.value = true
    await editor.state.save()
    assert.deepEqual(editor.requests, [{ accountIds: ['account-a'], customName: expected }])
    await vue.nextTick()
    assert.equal(editor.state.customName.value, '')
  }
  const editor = mountEditor(t)
  editor.state.open()
  editor.state.customName.value = 'Invalid\nname'
  editor.state.updateCustomName.value = true
  await editor.state.save()
  assert.equal(editor.requests.length, 0)
  assert.equal(editor.messages.error.length, 1)
  editor.state.updateCustomName.value = false
  editor.state.updateWeight.value = true
  await editor.state.save()
  assert.deepEqual(editor.requests, [{ accountIds: ['account-a'], weight: 17 }])
})

test('Excel batch update is opt-in and does not change ordinary scheduling', async (t) => {
  const editor = mountEditor(t)
  editor.state.open()
  assert.equal(editor.state.excelAvailable.value, true)
  assert.equal(editor.state.updateExcelEnabled.value, false)
  editor.state.excelEnabled.value = true
  assert.equal(editor.state.hasUpdates.value, false)
  editor.state.updateExcelEnabled.value = true
  await editor.state.save()
  assert.deepEqual(editor.requests, [{
    accountIds: ['account-a'],
    responsesUpstream: 'excel',
  }])
})

test('mixed providers cannot apply Excel settings but retain weight editing', async (t) => {
  const editor = mountEditor(t, {
    accounts: [account('account-a'), account('account-b', { provider: 'xai' })],
  })
  editor.state.open()
  assert.equal(editor.state.excelAvailable.value, false)
  editor.state.updateExcelEnabled.value = true
  editor.state.excelEnabled.value = true
  assert.equal(editor.state.hasUpdates.value, false)
  editor.state.updateWeight.value = true
  editor.state.weight.value = '20'
  await editor.state.save()
  assert.deepEqual(editor.requests, [{
    accountIds: ['account-a', 'account-b'],
    weight: 20,
  }])
})

test('Excel batch changes require known OAuth authentication for every selected account', async (t) => {
  for (const authenticationKind of ['api_key', undefined]) {
    const editor = mountEditor(t, {
      accounts: [account('account-a'), account('account-b', { authenticationKind })],
    })
    editor.state.open()
    assert.equal(editor.state.excelAvailable.value, false)
    editor.state.updateExcelEnabled.value = true
    editor.state.excelEnabled.value = true
    assert.equal(editor.state.hasUpdates.value, false)
    await editor.state.save()
    assert.equal(editor.requests.length, 0)
  }
})

test('batch editor requires fresh opt-ins initially, on every open and after closing', async (t) => {
  const { state } = mountEditor(t)
  assertNoUpdates(state)

  state.open()
  assert.equal(state.showBatchEditModal.value, true)
  assertNoUpdates(state)
  assert.equal(state.schedulingEnabled.value, true)
  assert.equal(state.concurrencyLimit.value, '8')
  assert.equal(state.weight.value, '17')
  assert.deepEqual(Array.from(state.selectedGroupIds.value), ['group-existing'])

  for (const field of updateFields)
    state[field].value = true
  state.schedulingEnabled.value = false
  state.concurrencyLimit.value = 'invalid'
  state.weight.value = 'invalid'
  state.selectedGroupIds.value = ['group-new']
  state.proxyMode.value = 'proxy'
  state.proxyId.value = 'proxy-old'
  assert.equal(state.hasUpdates.value, true)

  state.showBatchEditModal.value = false
  await vue.nextTick()
  assertNoUpdates(state)
  assert.equal(state.schedulingEnabled.value, true)
  assert.equal(state.concurrencyLimit.value, '')
  assert.equal(state.weight.value, '1')
  assert.deepEqual(Array.from(state.selectedGroupIds.value), [])
  assert.equal(state.proxyMode.value, 'preserve')
  assert.equal(state.proxyId.value, '')

  state.open()
  assertNoUpdates(state)
  assert.equal(state.concurrencyLimit.value, '8')
  assert.equal(state.weight.value, '17')
  assert.deepEqual(Array.from(state.selectedGroupIds.value), ['group-existing'])

  for (const field of updateFields)
    state[field].value = true
  state.proxyMode.value = 'direct'
  state.open()
  assertNoUpdates(state)
  assert.equal(state.proxyMode.value, 'preserve')
})

test('hasUpdates reacts to each opt-in and excludes an unchecked or preserved proxy', (t) => {
  const { state } = mountEditor(t)
  state.open()
  assert.ok(vue.isRef(state.hasUpdates))
  assert.ok(vue.isReadonly(state.hasUpdates), 'hasUpdates must expose derived state')

  for (const field of updateFields.slice(0, 4)) {
    state[field].value = true
    assert.equal(state.hasUpdates.value, true, field)
    state[field].value = false
    assert.equal(state.hasUpdates.value, false, field)
  }

  state.updateProxy.value = true
  assert.equal(state.hasUpdates.value, false, 'preserve alone changes nothing')
  for (const mode of ['direct', 'proxy']) {
    state.proxyMode.value = mode
    assert.equal(state.hasUpdates.value, true, mode)
    state.updateProxy.value = false
    assert.equal(state.hasUpdates.value, false, `unchecked ${mode}`)
    state.updateProxy.value = true
    assert.equal(state.hasUpdates.value, true)
  }
  state.proxyMode.value = 'preserve'
  assert.equal(state.hasUpdates.value, false)
  state.updateGroups.value = true
  assert.equal(state.hasUpdates.value, true, 'a non-proxy opt-in still applies')
})

test('saving without opt-ins warns and never validates stale values or sends a request', async (t) => {
  const editor = mountEditor(t)
  const { state } = editor
  state.open()
  await state.save()
  assert.equal(editor.messages.warning.length, 1)
  const noUpdatesMessage = editor.messages.warning[0]
  assert.ok(noUpdatesMessage.trim())
  assertNoRequest(editor)

  for (const field of updateFields)
    state[field].value = true
  state.concurrencyLimit.value = 'invalid'
  state.weight.value = '101'
  state.proxyMode.value = 'proxy'
  state.proxyId.value = ' \t '
  for (const field of updateFields)
    state[field].value = false

  await state.save()
  assert.deepEqual(editor.messages.warning, [noUpdatesMessage, noUpdatesMessage])
  assertNoRequest(editor)
  assert.equal(state.showBatchEditModal.value, true)
  assert.deepEqual(Array.from(editor.selectedIds.value), ['account-a'])

  state.updateProxy.value = true
  state.proxyMode.value = 'preserve'
  await state.save()
  assert.equal(state.hasUpdates.value, false)
  assert.deepEqual(editor.messages.warning, [noUpdatesMessage, noUpdatesMessage, noUpdatesMessage])
  assertNoRequest(editor)
})

test('every single field and combination sends exactly the opted-in patch after other fields are unchecked', async (t) => {
  const patches = [
    { enabled: false },
    { responsesUpstream: 'codex' },
    { excelModels: ['gpt-5.6-sol'], excelModelsFollowGlobal: false },
    { concurrencyLimit: 6 },
    { weight: 23 },
    { groupIds: ['group-new', 'group-other'] },
    { outboundProxyId: 'proxy-next' },
    { customName: null },
    { modelAccess: { mode: 'all', models: [] } },
  ]

  for (let mask = 1; mask < 2 ** updateFields.length; mask += 1) {
    const selected = updateFields.filter((_, index) => (mask & (1 << index)) !== 0)
    await t.test(selected.join(' + '), async (t) => {
      const editor = mountEditor(t)
      const { state } = editor
      state.open()
      for (const field of updateFields)
        state[field].value = true
      for (const field of updateFields)
        state[field].value = selected.includes(field)

      state.schedulingEnabled.value = false
      state.concurrencyLimit.value = state.updateConcurrencyLimit.value ? ' 6 ' : 'invalid'
      state.weight.value = state.updateWeight.value ? ' 23 ' : '101'
      state.selectedGroupIds.value = ['group-new', 'group-other', 'group-new']
      state.proxyMode.value = 'proxy'
      state.proxyId.value = state.updateProxy.value ? '  proxy-next \t ' : ' \t '
      assert.equal(state.hasUpdates.value, true)

      const expected = { accountIds: ['account-a'] }
      for (const [index, field] of updateFields.entries()) {
        if (selected.includes(field))
          Object.assign(expected, patches[index])
      }
      await state.save()
      await vue.nextTick()

      assert.deepEqual(editor.requests, [expected])
      assert.deepEqual(editor.messages.warning, [])
      assert.deepEqual(editor.messages.error, [])
      assert.equal(editor.messages.success.length, 1)
      assert.deepEqual(editor.reloads, { accounts: 1, groups: 1 })
      assert.equal(state.saving.value, false)
      assert.equal(state.showBatchEditModal.value, false)
      assert.equal(editor.selectedIds.value.size, 0)
      assertNoUpdates(state)
    })
  }
})

test('checked empty groups clear membership and checked empty concurrency restores the default', async (t) => {
  for (const input of ['', ' \t ']) {
    await t.test(`empty concurrency ${JSON.stringify(input)}`, async (t) => {
      const editor = mountEditor(t)
      const { state } = editor
      state.open()
      state.updateGroups.value = true
      state.selectedGroupIds.value = []
      state.updateConcurrencyLimit.value = true
      state.concurrencyLimit.value = input
      state.weight.value = 'invalid'

      await state.save()
      assert.deepEqual(editor.requests, [{
        accountIds: ['account-a'],
        concurrencyLimit: null,
        groupIds: [],
      }])
      assert.deepEqual(editor.messages.warning, [])
      assert.deepEqual(editor.messages.error, [])
    })
  }

  await t.test('unchecked empty values are omitted, not serialized as clears', async (t) => {
    const editor = mountEditor(t)
    const { state } = editor
    state.open()
    state.updateEnabled.value = true
    state.schedulingEnabled.value = false
    state.concurrencyLimit.value = ''
    state.selectedGroupIds.value = []
    await state.save()
    assert.deepEqual(editor.requests, [{ accountIds: ['account-a'], enabled: false }])
  })
})

test('checked scheduling fields use the real parser while unchecked invalid values are ignored', async (t) => {
  const cases = [
    ['updateConcurrencyLimit', 'concurrencyLimit', '0'],
    ['updateConcurrencyLimit', 'concurrencyLimit', '-1'],
    ['updateConcurrencyLimit', 'concurrencyLimit', '1.5'],
    ['updateConcurrencyLimit', 'concurrencyLimit', '4294967296'],
    ['updateConcurrencyLimit', 'concurrencyLimit', 'invalid'],
    ['updateWeight', 'weight', ''],
    ['updateWeight', 'weight', '0'],
    ['updateWeight', 'weight', '1.5'],
    ['updateWeight', 'weight', '101'],
    ['updateWeight', 'weight', 'invalid'],
  ]
  for (const [field, input, value] of cases) {
    await t.test(`${input}=${JSON.stringify(value)}`, async (t) => {
      const editor = mountEditor(t)
      const { state } = editor
      state.open()
      state[field].value = true
      state[input].value = value
      const parsed = schedulingForm.parseAccountSchedulingForm(
        field === 'updateConcurrencyLimit' ? value : '',
        field === 'updateWeight' ? value : '1',
      )
      assert.equal(parsed.valid, false)

      await state.save()
      assertNoRequest(editor)
      assert.deepEqual(editor.messages.warning, [parsed.message])
      assert.equal(state.showBatchEditModal.value, true)
      assert.equal(state[field].value, true)

      state[field].value = false
      state.updateEnabled.value = true
      state.schedulingEnabled.value = false
      await state.save()
      assert.deepEqual(editor.requests, [{ accountIds: ['account-a'], enabled: false }])
      assert.deepEqual(editor.messages.warning, [parsed.message], 'unchecked stale input must not validate again')
    })
  }
})

test('valid scheduling boundaries are serialized as numbers', async (t) => {
  for (const [concurrencyLimit, weight] of [['1', '1'], ['4294967295', '100']]) {
    await t.test(`concurrency=${concurrencyLimit}, weight=${weight}`, async (t) => {
      const editor = mountEditor(t)
      const { state } = editor
      state.open()
      state.updateConcurrencyLimit.value = true
      state.updateWeight.value = true
      state.concurrencyLimit.value = concurrencyLimit
      state.weight.value = weight
      await state.save()
      assert.deepEqual(editor.requests, [{
        accountIds: ['account-a'],
        concurrencyLimit: Number(concurrencyLimit),
        weight: Number(weight),
      }])
      assert.deepEqual(editor.messages.warning, [])
      assert.deepEqual(editor.messages.error, [])
    })
  }
})

test('proxy opt-in gates validation and serializes direct, proxy and preserve modes', async (t) => {
  const cases = [
    { updateProxy: true, mode: 'direct', id: 'stale-proxy', patch: { outboundProxyId: '' } },
    { updateProxy: true, mode: 'proxy', id: ' \t proxy-next \n ', patch: { outboundProxyId: 'proxy-next' } },
    { updateProxy: true, mode: 'preserve', id: '', patch: {} },
    { updateProxy: false, mode: 'direct', id: 'stale-proxy', patch: {} },
    { updateProxy: false, mode: 'proxy', id: '', patch: {} },
    { updateProxy: false, mode: 'proxy', id: 'stale-proxy', patch: {} },
  ]
  for (const { updateProxy, mode, id, patch } of cases) {
    await t.test(`${updateProxy ? 'checked' : 'unchecked'} ${mode} ${JSON.stringify(id)}`, async (t) => {
      const editor = mountEditor(t)
      const { state } = editor
      state.open()
      state.updateEnabled.value = true
      state.schedulingEnabled.value = false
      state.updateProxy.value = updateProxy
      state.proxyMode.value = mode
      state.proxyId.value = id
      await state.save()
      assert.deepEqual(editor.requests, [{ accountIds: ['account-a'], enabled: false, ...patch }])
      assert.deepEqual(editor.messages.warning, [])
      assert.deepEqual(editor.messages.error, [])
    })
  }

  await t.test('direct alone needs no proxy ID', async (t) => {
    const editor = mountEditor(t)
    const { state } = editor
    state.open()
    state.updateProxy.value = true
    state.proxyMode.value = 'direct'
    state.proxyId.value = ''
    await state.save()
    assert.deepEqual(editor.requests, [{ accountIds: ['account-a'], outboundProxyId: '' }])
    assert.deepEqual(editor.messages.warning, [])
  })

  await t.test('checked proxy rejects a blank ID until it is supplied', async (t) => {
    const editor = mountEditor(t)
    const { state } = editor
    state.open()
    state.updateProxy.value = true
    state.proxyMode.value = 'proxy'
    for (const id of ['', ' \t\n ']) {
      state.proxyId.value = id
      await state.save()
      assertNoRequest(editor)
    }
    assert.equal(editor.messages.warning.length, 2)
    assert.ok(editor.messages.warning.every(message => message.trim()))
    assert.equal(state.showBatchEditModal.value, true)
    state.proxyId.value = ' proxy-next '
    await state.save()
    assert.deepEqual(editor.requests, [{ accountIds: ['account-a'], outboundProxyId: 'proxy-next' }])
    assert.equal(editor.messages.warning.length, 2)
  })
})

test('cached selections from earlier pages remain in the batch without including deselected or unselected rows', async (t) => {
  const first = account('account-a')
  const second = account('account-b', { groups: [{ id: 'group-existing' }, { id: 'group-extra' }] })
  const editor = mountEditor(t, {
    accounts: [first, account('account-removed')],
    selectedIds: ['account-a', 'account-removed'],
  })
  const { state } = editor

  editor.accounts.value = [second, account('account-unselected')]
  editor.selectedIds.value = new Set(['account-a', 'account-b'])
  await vue.nextTick()
  state.open()
  assertNoUpdates(state)
  assert.equal(state.concurrencyLimit.value, '8')
  assert.equal(state.weight.value, '17')
  assert.deepEqual(Array.from(state.selectedGroupIds.value), ['group-existing'])

  state.updateEnabled.value = true
  state.schedulingEnabled.value = false
  await state.save()
  assert.deepEqual(editor.requests, [{ accountIds: ['account-a', 'account-b'], enabled: false }])
  assert.deepEqual(editor.messages.error, [])

  editor.accounts.value = [account('account-c', { enabled: false, concurrencyLimit: null, weight: 2, groups: [] })]
  editor.selectedIds.value = new Set(['account-c'])
  state.open()
  assertNoUpdates(state)
  assert.equal(state.schedulingEnabled.value, false)
  assert.equal(state.concurrencyLimit.value, '')
  assert.equal(state.weight.value, '2')
  assert.deepEqual(Array.from(state.selectedGroupIds.value), [])
})

test('opening mixed accounts does not turn their displayed defaults into implicit updates', async (t) => {
  const editor = mountEditor(t, {
    accounts: [
      account('account-a'),
      account('account-b', { enabled: false, concurrencyLimit: null, weight: 2, groups: [] }),
    ],
  })
  const { state } = editor
  state.open()
  assertNoUpdates(state)
  assert.equal(state.schedulingEnabled.value, false)
  assert.equal(state.concurrencyLimit.value, '')
  assert.equal(state.weight.value, '1')
  assert.deepEqual(Array.from(state.selectedGroupIds.value), [])
  state.updateWeight.value = true
  state.weight.value = '5'
  await state.save()
  assert.deepEqual(editor.requests, [{ accountIds: ['account-a', 'account-b'], weight: 5 }])
  assert.deepEqual(editor.messages.warning, [])
})

test('real async action blocks duplicate saves and resets opt-ins after success', async (t) => {
  let complete
  const editor = mountEditor(t, {
    batchUpdateAccounts: () => new Promise((resolve) => { complete = resolve }),
  })
  const { state } = editor
  state.open()
  state.updateEnabled.value = true
  state.schedulingEnabled.value = false
  const saving = state.save()
  try {
    assert.equal(state.saving.value, true)
    assert.equal(editor.requests.length, 1)
    await state.save()
    assert.equal(editor.requests.length, 1)
    assert.equal(state.updateEnabled.value, true)
  }
  finally {
    complete()
    await saving
  }
  await vue.nextTick()
  assert.equal(state.saving.value, false)
  assert.equal(state.showBatchEditModal.value, false)
  assertNoUpdates(state)
  assert.deepEqual(editor.reloads, { accounts: 1, groups: 1 })
})

test('failed saves retain the selection and opt-ins for retry through the real async action', async (t) => {
  let attempts = 0
  const editor = mountEditor(t, {
    batchUpdateAccounts: async () => {
      attempts += 1
      if (attempts === 1)
        throw new Error('batch update failed')
    },
  })
  const { state } = editor
  state.open()
  state.updateGroups.value = true
  state.selectedGroupIds.value = []
  await state.save()
  await vue.nextTick()
  assert.equal(state.saving.value, false)
  assert.equal(state.showBatchEditModal.value, true)
  assert.equal(state.updateGroups.value, true)
  assert.equal(state.hasUpdates.value, true)
  assert.deepEqual(Array.from(editor.selectedIds.value), ['account-a'])
  assert.deepEqual(editor.messages.error, ['batch update failed'])
  assert.deepEqual(editor.messages.success, [])
  assert.deepEqual(editor.reloads, { accounts: 1, groups: 0 })

  await state.save()
  await vue.nextTick()
  assert.deepEqual(editor.requests, [
    { accountIds: ['account-a'], groupIds: [] },
    { accountIds: ['account-a'], groupIds: [] },
  ])
  assert.deepEqual(editor.reloads, { accounts: 2, groups: 1 })
  assertNoUpdates(state)
})

test('an empty account selection cannot open or submit a batch', async (t) => {
  const editor = mountEditor(t, { selectedIds: [] })
  const { state } = editor
  state.open()
  assert.equal(state.showBatchEditModal.value, false)
  assertNoUpdates(state)
  state.updateEnabled.value = true
  await state.save()
  assertNoRequest(editor)
})

for (const failure of ['business', 'http']) {
  test(`real batch API ${failure} failure notifies once and silently rereads without discarding opt-ins`, async (t) => {
    const calls = []
    const editor = mountEditor(t, {
      adapter: async (config) => {
        calls.push(config)
        const response = {
          config,
          status: failure === 'business' ? 200 : 503,
          statusText: '',
          headers: {},
          data: { code: 40901, message: 'synthetic batch conflict', data: null },
        }
        if (failure === 'http')
          throw new axios.AxiosError('request failed', 'ERR_BAD_RESPONSE', config, undefined, response)
        return response
      },
    })
    const { state } = editor
    state.open()
    state.updateWeight.value = true
    state.weight.value = '23'
    await state.save()
    await vue.nextTick()

    assert.equal(calls.length, 1, 'neither error handling nor rereading may replay the mutation')
    assert.equal(calls[0].url, '/api/admin/accounts/batch-update')
    assert.equal(calls[0].method, 'post')
    assert.deepEqual(JSON.parse(calls[0].data), { accountIds: ['account-a'], weight: 23 })
    assert.deepEqual(editor.messages.error, ['synthetic batch conflict'])
    assert.deepEqual(editor.messages.success, [])
    assert.deepEqual(editor.reloads, { accounts: 1, groups: 0 })
    assert.equal(editor.reloadOptions[0].silent, true)
    assert.equal(state.saving.value, false)
    assert.equal(state.showBatchEditModal.value, true)
    assert.equal(state.updateWeight.value, true)
    assert.equal(state.weight.value, '23')
    assert.deepEqual(Array.from(editor.selectedIds.value), ['account-a'])
  })
}
