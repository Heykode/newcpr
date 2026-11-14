/* eslint-disable test/no-import-node-test -- uses the project's Node test runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'
import { createTestRequire as createRequire } from './helpers/i18n.mjs'

const require = createRequire(import.meta.url)
function load(path, dependencies = {}) {
  dependencies['@/api/modules/ipv6-egress'] ??= { getIpv6Egress: async () => ({ accountOverrides: {} }) }
  if (path !== '../src/utils/account-egress.ts')
    dependencies['@/utils/account-egress'] = load('../src/utils/account-egress.ts')
  const exports = {}
  const { outputText } = ts.transpileModule(readFileSync(new URL(path, import.meta.url), 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, { exports, TextEncoder, AbortController, require: name => dependencies[name] ?? (name.endsWith('/utils/purchaseCost') ? load('../src/views/accounts/utils/purchaseCost.ts') : name === '@/utils/excel-defaults' ? load('../src/utils/excel-defaults.ts') : require(name)) })
  return exports
}

test('Excel editor preserves omitted values and sends only an explicit route change for OAuth', async (t) => {
  const updates = []
  const toast = { warning() {}, error() {}, success() {} }
  const asyncAction = load('../src/composables/useAsyncAction.ts', {
    vue,
    '@/api/request': load('../src/api/error.ts'),
    '@/components/base/BaseToast': { toast },
    '@/utils/async': load('../src/utils/async.ts'),
  })
  const { useAccountEditor } = load('../src/views/accounts/composables/useAccountEditor.ts', {
    vue,
    '@/api': { updateAccount: async payload => updates.push(structuredClone(payload)) },
    '@/components/base/BaseToast': { toast },
    '@/composables/useAsyncAction': asyncAction,
    '@/utils/account-name': load('../src/utils/account-name.ts'),
    '../utils/schedulingForm': load('../src/views/accounts/utils/schedulingForm.ts'),
    '@/utils/excel-settings': load('../src/utils/excel-settings.ts', { '@/views/accounts/utils/schedulingForm': load('../src/views/accounts/utils/schedulingForm.ts') }),
    '../utils/modelAccess': load('../src/views/accounts/utils/modelAccess.ts'),
  })
  const accounts = vue.ref([{
    id: 'acct_fixture',
    provider: 'openai',
    authenticationKind: 'oauth',
    enabled: true,
    turnStateInjectionEnabled: false,
    responsesUpstream: 'excel',
    concurrencyLimit: null,
    weight: 1,
    groups: [],
  }])
  const scope = vue.effectScope()
  t.after(() => scope.stop())
  const state = scope.run(() => useAccountEditor({
    accounts,
    reloadAccounts: async () => {},
    reloadGroups: async () => {},
  }))
  state.open(accounts.value[0])
  assert.equal(state.excelEnabled.value, true)
  assert.equal('excelModels' in state, false)
  await state.save()
  assert.equal(Object.hasOwn(updates[0], 'responsesUpstream'), false)
  assert.equal(Object.hasOwn(updates[0], 'excelRecovery'), false)
  state.open(accounts.value[0])
  state.excelEnabled.value = false
  await state.save()
  assert.equal(updates[1].responsesUpstream, 'codex')
  accounts.value[0].responsesUpstream = 'codex'
  state.open(accounts.value[0])
  state.excelEnabled.value = true
  await state.save()
  assert.equal(updates[2].responsesUpstream, 'excel')
  accounts.value[0].authenticationKind = 'api_key'
  state.open(accounts.value[0])
  state.excelEnabled.value = true
  await state.save()
  assert.equal(Object.hasOwn(updates[3], 'responsesUpstream'), false)
  accounts.value[0].authenticationKind = 'none'
  state.open(accounts.value[0])
  state.excelEnabled.value = true
  await state.save()
  assert.equal(updates.at(-1).responsesUpstream, 'excel')
  accounts.value[0].authenticationKind = 'oauth'
  accounts.value[0].excelModelsFollowGlobal = true
  accounts.value[0].excelModels = ['invalid/model']
  state.open(accounts.value[0])
  await state.save()
  assert.equal('excelModelsFollowGlobal' in updates.at(-1), false)
  assert.equal('excelModels' in updates.at(-1), false)
  state.open(accounts.value[0])
  assert.equal(state.excelCacheCreationAsInput.value, false)
  assert.equal('excelCacheCreationAsInput' in updates.at(-1), false)
  state.excelEnabled.value = true
  state.excelCacheCreationAsInput.value = true
  await state.save()
  assert.equal(updates.at(-1).excelCacheCreationAsInput, true)
  assert.equal(updates.at(-1).responsesUpstream, 'excel')
  state.open(accounts.value[0])
  assert.equal(state.excel403Action.value, 'none')
  state.excelEnabled.value = true
  state.excel403Action.value = 'pause_account'
  await state.save()
  assert.equal(updates.at(-1).excel403Action, 'pause_account')
  accounts.value[0].responsesUpstream = 'excel'
  accounts.value[0].excel403Action = 'pause_account'
  state.open(accounts.value[0])
  state.excelEnabled.value = false
  await state.save()
  assert.equal(updates.at(-1).responsesUpstream, 'codex')
  assert.equal(updates.at(-1).excel403Action, 'none')
  state.open(accounts.value[0])
  state.excelCacheCreationAsInput.value = true
  state.excelEnabled.value = false
  await state.save()
  assert.equal(updates.at(-1).excelCacheCreationAsInput, true)
  state.open(accounts.value[0])
  state.excel403Action.value = 'disable_excel'
  await state.save()
  assert.equal(updates.at(-1).excel403Action, 'disable_excel')
  state.open(accounts.value[0])
  assert.equal(state.excelIgnoreEncryptedContent.value, false)
  state.excelIgnoreEncryptedContent.value = true
  await state.save()
  assert.equal(updates.at(-1).excelIgnoreEncryptedContent, true)
  accounts.value[0].excelIgnoreEncryptedContent = true
  state.open(accounts.value[0])
  assert.equal(state.excelIgnoreEncryptedContent.value, true)
  await state.save()
  assert.equal('excelIgnoreEncryptedContent' in updates.at(-1), false)
  state.open(accounts.value[0])
  state.excelIgnoreEncryptedContent.value = false
  await state.save()
  assert.equal(updates.at(-1).excelIgnoreEncryptedContent, false)
  state.open(accounts.value[0])
  state.excelEnabled.value = false
  await state.save()
  assert.equal(updates.at(-1).responsesUpstream, 'codex')
  assert.equal(updates.at(-1).excelIgnoreEncryptedContent, false)
  accounts.value[0].authenticationKind = 'api_key'
  state.open(accounts.value[0])
  state.excelIgnoreEncryptedContent.value = true
  await state.save()
  assert.equal('excelIgnoreEncryptedContent' in updates.at(-1), false)
})

test('Excel import respects route settings without a separate model list', () => {
  const scheduling = load('../src/views/accounts/utils/schedulingForm.ts')
  const creation = load('../src/views/accounts/components/AccountCreateModal/model.ts', {
    '@/utils/excel-settings': load('../src/utils/excel-settings.ts', { '@/views/accounts/utils/schedulingForm': scheduling }),
    '@/utils/account-name': load('../src/utils/account-name.ts'),
    '../../utils/schedulingForm': scheduling,
    '../../utils/modelAccess': load('../src/views/accounts/utils/modelAccess.ts'),
  })
  const form = creation.emptyAccountCreateForm()
  form.provider = 'openai'
  assert.equal('responsesUpstream' in creation.accountImportSettings(form), false)
  form.applyExcel = true
  form.excelEnabled = true
  let settings = creation.accountImportSettings(form)
  assert.equal('excelModelsFollowGlobal' in settings, false)
  assert.equal(settings.responsesUpstream, 'excel')
  assert.equal('excelModels' in settings, false)
  assert.equal(settings.excelCacheCreationAsInput, true)
  assert.equal(settings.excel403Action, 'none')
  form.excel403Action = 'pause_account'
  assert.equal(creation.accountImportSettings(form).excel403Action, 'pause_account')
  form.excelCacheCreationAsInput = true
  assert.equal(creation.accountImportSettings(form).excelCacheCreationAsInput, true)
  assert.equal('responsesUpstream' in creation.accountImportSettings(form, 'xai'), false)
  form.excelModelsFollowGlobal = false
  form.excelModels = 'gpt-5.6-sol, gpt-6-astra'
  settings = creation.accountImportSettings(form)
  assert.equal('excelModels' in settings, false)
  form.excelEnabled = false
  assert.equal(creation.accountImportSettings(form).excelCacheCreationAsInput, true)
})
