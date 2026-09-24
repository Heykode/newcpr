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

test('a committed single-account save finishes before slow or failed list reloads', async (t) => {
  const reload = Promise.withResolvers()
  const errors = []
  const successes = []
  const updates = []
  const toast = { error: message => errors.push(message), success: message => successes.push(message) }
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
    id: 'acct_selected',
    provider: 'openai',
    enabled: true,
    turnStateInjectionEnabled: false,
    concurrencyLimit: null,
    weight: 1,
    groups: [],
  }])
  const scope = vue.effectScope()
  t.after(() => scope.stop())
  const state = scope.run(() => useAccountEditor({
    accounts,
    reloadAccounts: () => reload.promise,
    reloadGroups: async () => { throw new Error('list refresh failed after commit') },
  }))
  state.open(accounts.value[0])
  state.weight.value = '9'
  const saving = state.save()
  try {
    await new Promise(resolve => setImmediate(resolve))
    assert.equal(state.saving.value, false)
    assert.equal(state.showEditModal.value, false)
    assert.equal(updates.length, 1)
    assert.equal(updates[0].weight, 9)
    assert.equal(successes.length, 1)
    assert.deepEqual(errors, [])
  }
  finally {
    reload.resolve()
    await saving
  }
})
