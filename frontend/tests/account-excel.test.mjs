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
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, { exports, TextEncoder, require: name => dependencies[name] ?? require(name) })
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
  await state.save()
  assert.equal(Object.hasOwn(updates[0], 'responsesUpstream'), false)
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
  accounts.value[0].authenticationKind = 'oauth'
  state.open(accounts.value[0])
  state.excelModels.value = 'gpt-5.6-sol, gpt-6-astra, gpt-5.6-sol'
  await state.save()
  assert.deepEqual(updates[4].excelModels, ['gpt-5.6-sol', 'gpt-6-astra'])
  state.open(accounts.value[0])
  state.excelModels.value = ''
  await state.save()
  assert.deepEqual(updates[5].excelModels, [])
  state.open(accounts.value[0])
  state.excelModels.value = 'invalid/model'
  await state.save()
  assert.equal(updates.length, 6)
})
