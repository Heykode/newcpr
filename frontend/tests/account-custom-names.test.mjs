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
const names = load('../src/utils/account-name.ts')
const scheduling = load('../src/views/accounts/utils/schedulingForm.ts')
const modelAccess = load('../src/views/accounts/utils/modelAccess.ts')
const creation = load('../src/views/accounts/components/AccountCreateModal/model.ts', {
  '@/utils/excel-settings': load('../src/utils/excel-settings.ts', { '@/views/accounts/utils/schedulingForm': scheduling }),
  '../../utils/schedulingForm': scheduling,
  '../../utils/modelAccess': modelAccess,
  '@/utils/account-name': names,
})

test('names trim, clear, count Unicode characters and reject control characters', () => {
  for (const input of [undefined, null, '', '   '])
    assert.equal(names.normalizeAccountName(input), null)
  assert.equal(names.normalizeAccountName('  Batch A  '), 'Batch A')
  assert.equal(names.normalizeAccountName('名'.repeat(128)), '名'.repeat(128))
  assert.equal(names.normalizeAccountName('\u{1F511}'.repeat(128)), '\u{1F511}'.repeat(128))
  for (const value of ['a'.repeat(129), 'bad\nname', '\tname', 'name\u007F'])
    assert.throws(() => names.normalizeAccountName(value))
})

test('imports omit blank names, preserve other settings and reject invalid names', () => {
  const form = creation.emptyAccountCreateForm()
  assert.equal(form.customName, '')
  form.provider = 'openai'
  const original = creation.accountImportSettings(form)
  assert.equal('customName' in original, false)
  form.customName = '  Import batch  '
  const settings = creation.accountImportSettings(form)
  assert.equal(JSON.stringify(settings), JSON.stringify({ customName: 'Import batch', ...original }))
  form.customName = ' '.repeat(5)
  assert.equal('customName' in creation.accountImportSettings(form), false)
  form.customName = 'bad\nname'
  assert.throws(() => creation.accountImportSettings(form))
})

test('single editor only submits a changed name and preserves concurrent external renames', async (t) => {
  const accounts = vue.ref([{
    id: 'synthetic-account',
    customName: 'Original',
    provider: 'openai',
    enabled: true,
    turnStateInjectionEnabled: false,
    concurrencyLimit: null,
    weight: 1,
    groups: [],
  }])
  const requests = []
  const errors = []
  const asyncAction = load('../src/composables/useAsyncAction.ts', {
    vue,
    '@/api/request': load('../src/api/error.ts'),
    '@/components/base/BaseToast': { toast: { error: message => errors.push(message) } },
    '@/utils/async': load('../src/utils/async.ts'),
  })
  const { useAccountEditor } = load('../src/views/accounts/composables/useAccountEditor.ts', {
    vue,
    '@/api': { updateAccount: async payload => requests.push(structuredClone(payload)) },
    '@/components/base/BaseToast': { toast: { warning() {}, success() {} } },
    '@/composables/useAsyncAction': asyncAction,
    '@/utils/account-name': names,
    '../utils/schedulingForm': scheduling,
    '../utils/modelAccess': modelAccess,
  })
  const scope = vue.effectScope()
  t.after(() => scope.stop())
  const editor = scope.run(() => useAccountEditor({
    accounts,
    reloadAccounts: async () => {},
    reloadGroups: async () => {},
  }))
  editor.open(accounts.value[0])
  accounts.value[0].customName = 'External rename'
  await editor.save()
  assert.equal('customName' in requests[0], false)
  await vue.nextTick()
  editor.open(accounts.value[0])
  editor.customName.value = '   '
  await editor.save()
  assert.equal(requests[1].customName, null)
  await vue.nextTick()
  editor.open(accounts.value[0])
  editor.customName.value = '  New name  '
  await editor.save()
  assert.equal(requests[2].customName, 'New name')
  await vue.nextTick()
  editor.open(accounts.value[0])
  editor.customName.value = 'bad\nname'
  await editor.save()
  assert.equal(requests.length, 3)
  assert.equal(errors.length, 1)
  assert.equal(editor.showEditModal.value, true)
})
