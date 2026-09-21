/* eslint-disable test/no-import-node-test -- follows the project's Node regression runner. */
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
const policy = load('../src/views/accounts/utils/modelAccess.ts')
const scheduling = load('../src/views/accounts/utils/schedulingForm.ts')
const names = load('../src/utils/account-name.ts')
const creation = load('../src/views/accounts/components/AccountCreateModal/model.ts', {
  '../../utils/schedulingForm': scheduling,
  '../../utils/modelAccess': policy,
  '@/utils/account-name': names,
})

test('model IDs are exact, bounded by bytes and reject wildcard or reserved IDs', () => {
  for (const id of ['', ' ', ' model-a', 'model-*', '__reserved', 'bad\nid', 'x'.repeat(257), '模'.repeat(86)])
    assert.ok(policy.accountModelIdError(id), id)
  for (const id of ['model-a', 'x'.repeat(256), '模'.repeat(85)])
    assert.equal(policy.accountModelIdError(id), undefined)
  for (const mode of ['allowlist', 'denylist']) {
    assert.ok(policy.accountModelAccessError({ mode, models: [] }))
    assert.ok(policy.accountModelAccessError({ mode, models: Array.from({ length: 257 }, (_, i) => `model-${i}`) }))
    assert.equal(policy.accountModelAccessError({ mode, models: ['model-a'] }), undefined)
  }
  assert.equal(policy.accountModelAccessError(undefined), undefined)
})

test('imports preserve existing policies by default and clone explicit settings', () => {
  const form = creation.emptyAccountCreateForm()
  form.provider = 'openai'
  assert.equal('modelAccess' in creation.accountImportSettings(form), false)
  form.modelAccess = { mode: 'allowlist', models: ['model-a'] }
  const settings = creation.accountImportSettings(form)
  assert.deepEqual(JSON.parse(JSON.stringify(settings.modelAccess)), form.modelAccess)
  form.modelAccess.models.push('model-b')
  assert.equal(settings.modelAccess.models.length, 1)
  form.modelAccess = { mode: 'denylist', models: [] }
  assert.throws(() => creation.accountImportSettings(form))
  form.modelAccess = { mode: 'all', models: [] }
  assert.equal(creation.accountImportSettings(form).modelAccess.mode, 'all')
})

test('single editing only sends a changed policy and does not mutate the account row', async (t) => {
  const accounts = vue.ref([{
    id: 'acct_model_ui',
    provider: 'openai',
    enabled: true,
    turnStateInjectionEnabled: true,
    concurrencyLimit: 3,
    weight: 2,
    groups: [],
    modelAccess: { mode: 'allowlist', models: ['model-a'] },
  }])
  const requests = []
  const warnings = []
  const toast = { warning: value => warnings.push(value), success() {}, error: error => assert.fail(error) }
  const asyncAction = load('../src/composables/useAsyncAction.ts', {
    vue,
    '@/api/request': load('../src/api/error.ts'),
    '@/components/base/BaseToast': { toast },
    '@/utils/async': load('../src/utils/async.ts'),
  })
  const { useAccountEditor } = load('../src/views/accounts/composables/useAccountEditor.ts', {
    vue,
    '@/api': { updateAccount: async payload => requests.push(structuredClone(payload)) },
    '@/components/base/BaseToast': { toast },
    '@/composables/useAsyncAction': asyncAction,
    '@/utils/account-name': names,
    '../utils/modelAccess': policy,
    '../utils/schedulingForm': scheduling,
  })
  const scope = vue.effectScope()
  t.after(() => scope.stop())
  const editor = scope.run(() => useAccountEditor({
    accounts,
    reloadAccounts: async () => {},
    reloadGroups: async () => {},
  }))
  editor.open(accounts.value[0])
  accounts.value[0].modelAccess.models = ['model-external']
  await editor.save()
  assert.equal('modelAccess' in requests[0], false)
  await vue.nextTick()
  editor.open(accounts.value[0])
  editor.modelAccess.value.models.push('model-b')
  assert.deepEqual([...accounts.value[0].modelAccess.models], ['model-external'])
  await editor.save()
  assert.deepEqual(requests[1].modelAccess, { mode: 'allowlist', models: ['model-external', 'model-b'] })
  await vue.nextTick()
  editor.open(accounts.value[0])
  editor.modelAccess.value = { mode: 'denylist', models: [] }
  await editor.save()
  assert.equal(requests.length, 2)
  assert.equal(warnings.length, 1)
  editor.modelAccess.value = { mode: 'all', models: [] }
  await editor.save()
  assert.deepEqual(requests[2].modelAccess, { mode: 'all', models: [] })
})
