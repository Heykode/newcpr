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
  runInNewContext(outputText, {
    exports,
    TextEncoder,
    require: name => dependencies[name] ?? (name.startsWith('@/utils/') ? load(`../src/utils/${name.slice('@/utils/'.length)}.ts`, dependencies) : require(name)),
  })
  return exports
}
const { accountEgressPatch, accountIpv6Modes } = load('../src/utils/account-egress.ts')
const plain = value => JSON.parse(JSON.stringify(value))
const draft = (proxyMode, egressMode = 'random_ipv6_reuse') => ({ proxyMode, egressMode, proxyId: ' old-proxy ' })

test('one egress choice replaces only the relevant routing settings', () => {
  assert.deepEqual(plain(accountEgressPatch(draft('preserve'), true)), {})
  for (const mode of ['mihomo', 'proxy_pool'])
    assert.deepEqual(plain(accountEgressPatch(draft(mode), true)), { requestProxySource: mode })
  assert.deepEqual(plain(accountEgressPatch(draft('inherit'), true)), { requestProxySource: 'account', outboundProxyId: '', egressMode: null })
  assert.deepEqual(plain(accountEgressPatch(draft('direct'), true)), { requestProxySource: 'account', outboundProxyId: '', egressMode: 'unchanged' })
  assert.deepEqual(plain(accountEgressPatch(draft('proxy'), true)), { requestProxySource: 'account', outboundProxyId: 'old-proxy', egressMode: 'unchanged' })
  for (const mode of accountIpv6Modes)
    assert.deepEqual(plain(accountEgressPatch(draft('ipv6', mode), true)), { requestProxySource: 'account', outboundProxyId: '', egressMode: mode })
  assert.throws(() => accountEgressPatch(draft('ipv6', 'invalid'), true))
  assert.throws(() => accountEgressPatch({ ...draft('proxy'), proxyId: ' ' }, true))
  assert.throws(() => accountEgressPatch(draft('unknown'), true))
})

test('non-OpenAI modes cannot accidentally submit OpenAI-only fields', () => {
  assert.deepEqual(plain(accountEgressPatch(draft('direct'), false)), { requestProxySource: 'account', outboundProxyId: '' })
  assert.deepEqual(plain(accountEgressPatch(draft('proxy'), false)), { requestProxySource: 'account', outboundProxyId: 'old-proxy' })
  for (const mode of ['inherit', 'mihomo', 'proxy_pool', 'ipv6'])
    assert.throws(() => accountEgressPatch(draft(mode), false))
})

test('all OpenAI enrollment modes carry the same egress settings independently of Excel', () => {
  const scheduling = load('../src/views/accounts/utils/schedulingForm.ts')
  const { emptyAccountCreateForm, accountImportSettings } = load('../src/views/accounts/components/AccountCreateModal/model.ts', {
    '../../utils/schedulingForm': scheduling,
    '@/views/accounts/utils/schedulingForm': scheduling,
    '../../utils/modelAccess': load('../src/views/accounts/utils/modelAccess.ts'),
  })
  for (const mode of ['oauth', 'access_token', 'refresh_token', 'json', 'two_fa']) {
    for (const excelEnabled of [false, true]) {
      const form = { ...emptyAccountCreateForm(), provider: 'openai', mode, applyExcel: true, excelEnabled, ...draft('ipv6') }
      const settings = accountImportSettings(form)
      assert.equal(settings.egressMode, 'random_ipv6_reuse')
      assert.equal(settings.requestProxySource, 'account')
      assert.equal(settings.responsesUpstream, excelEnabled ? 'excel' : 'codex')
      assert.equal('outboundProxyId' in settings, false, 'proxy ID remains the import request field, not settings')
      form.proxyMode = 'mihomo'
      const managed = accountImportSettings(form)
      assert.equal(managed.requestProxySource, 'mihomo')
      assert.equal('egressMode' in managed, false)
    }
  }
})

test('single account saves egress atomically and cancellation never persists a draft', async (t) => {
  const requests = []
  const toast = { success() {}, warning() {}, error() {} }
  const scheduling = load('../src/views/accounts/utils/schedulingForm.ts')
  const dependencies = {
    vue,
    '@/views/accounts/utils/schedulingForm': scheduling,
    '../utils/schedulingForm': scheduling,
    '../utils/modelAccess': load('../src/views/accounts/utils/modelAccess.ts'),
    '@/components/base/BaseToast': { toast },
    '@/api/request': load('../src/api/error.ts'),
    '@/api': { updateAccount: async body => requests.push(structuredClone(body)) },
  }
  dependencies['@/composables/useAsyncAction'] = load('../src/composables/useAsyncAction.ts', dependencies)
  const { useAccountEditor } = load('../src/views/accounts/composables/useAccountEditor.ts', dependencies)
  const accounts = vue.ref([{ id: 'acct_test', provider: 'openai', authenticationKind: 'oauth', enabled: true, weight: 1, concurrencyLimit: null, groups: [], requestProxySource: 'mihomo', responsesUpstream: 'excel' }])
  const scope = vue.effectScope()
  t.after(() => scope.stop())
  const editor = scope.run(() => useAccountEditor({ accounts, reloadAccounts: async () => {}, reloadGroups: async () => {} }))
  editor.open(accounts.value[0])
  editor.proxyMode.value = 'ipv6'
  editor.egressMode.value = 'fixed_ipv6_fresh'
  editor.showEditModal.value = false
  await vue.nextTick()
  assert.equal(requests.length, 0)
  editor.open(accounts.value[0])
  await editor.save()
  for (const key of ['egressMode', 'outboundProxyId', 'requestProxySource'])
    assert.equal(key in requests[0], false, `unchanged save omits ${key}`)
  editor.open(accounts.value[0])
  editor.proxyMode.value = 'ipv6'
  editor.egressMode.value = 'random_ipv6_fresh'
  editor.proxyId.value = 'stale-proxy'
  await editor.save()
  assert.equal(requests.length, 2)
  assert.equal(requests[1].egressMode, 'random_ipv6_fresh')
  assert.equal(requests[1].outboundProxyId, '')
  assert.equal(requests[1].requestProxySource, 'account')
  assert.equal('responsesUpstream' in requests[1], false)
})
