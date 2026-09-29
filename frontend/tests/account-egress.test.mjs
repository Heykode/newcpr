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
  dependencies['@/api/modules/ipv6-egress'] ??= { getIpv6Egress: async () => ({ accountOverrides: {} }) }
  const exports = {}
  const { outputText } = ts.transpileModule(readFileSync(new URL(path, import.meta.url), 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, {
    exports,
    TextEncoder,
    AbortController,
    require: name => dependencies[name] ?? (name.startsWith('@/utils/') ? load(`../src/utils/${name.slice('@/utils/'.length)}.ts`, dependencies) : require(name)),
  })
  return exports
}
const { accountEgressPatch, accountIpv6Modes, accountEgressFromAccount, sameAccountEgress } = load('../src/utils/account-egress.ts')
const plain = value => JSON.parse(JSON.stringify(value))
const draft = (proxyMode, egressMode = 'random_ipv6_reuse') => ({ proxyMode, egressMode, proxyId: ' old-proxy ' })
const settleReads = () => new Promise(resolve => setImmediate(resolve))

test('saved egress projection preserves pool, proxy, override and global precedence', () => {
  const account = { id: 'acct_test', provider: 'openai' }
  const config = { defaultMode: 'random_ipv6_reuse', accountOverrides: {} }
  assert.equal(accountEgressFromAccount(account), undefined)
  assert.equal(accountEgressFromAccount(account, config).proxyMode, 'inherit')
  for (const mode of [null, 'unchanged', ...accountIpv6Modes]) {
    config.accountOverrides.acct_test = mode
    const saved = accountEgressFromAccount(account, config)
    assert.equal(saved.proxyMode, mode === null ? 'inherit' : mode === 'unchanged' ? 'direct' : 'ipv6')
    if (mode && mode !== 'unchanged')
      assert.equal(saved.egressMode, mode)
    const proxied = { ...account, outboundProxyEndpoint: 'http://proxy.example:8080' }
    assert.equal(accountEgressFromAccount(proxied, config).proxyMode, 'proxy')
    assert.equal(accountEgressFromAccount(proxied, config).proxyId, '', 'never guess an ID from the redacted endpoint')
    for (const source of ['mihomo', 'proxy_pool'])
      assert.equal(accountEgressFromAccount({ ...proxied, requestProxySource: source }, config).proxyMode, source)
  }
  config.accountOverrides.acct_test = 'future_mode'
  assert.equal(accountEgressFromAccount(account, config), undefined, 'unknown policies must not become a fake default')
  assert.equal(accountEgressFromAccount({ ...account, provider: 'anthropic' }).proxyMode, 'direct')
})

test('egress comparison ignores inactive drafts, including dormant proxy and IPv6 fields', () => {
  for (const mode of ['mihomo', 'proxy_pool', 'inherit', 'direct'])
    assert.equal(sameAccountEgress(draft(mode), { proxyMode: mode, proxyId: 'another', egressMode: 'fixed_ipv6_fresh' }), true)
  assert.equal(sameAccountEgress(draft('proxy'), { ...draft('proxy'), proxyId: 'old-proxy', egressMode: 'fixed_ipv6_fresh' }), true)
  assert.equal(sameAccountEgress(draft('proxy'), { ...draft('proxy'), proxyId: 'new-proxy' }), false)
  assert.equal(sameAccountEgress(draft('ipv6'), { ...draft('ipv6'), proxyId: 'another' }), true)
  assert.equal(sameAccountEgress(draft('ipv6'), draft('ipv6', 'fixed_ipv6_fresh')), false)
  assert.equal(sameAccountEgress(draft('inherit'), draft('direct')), false)
})

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

function editorHarness(t, overrides = {}, readConfig = async () => ({ accountOverrides: {} })) {
  const requests = []
  const reads = []
  const errors = []
  const toast = { success() {}, warning: message => errors.push(message), error: message => errors.push(message) }
  const scheduling = load('../src/views/accounts/utils/schedulingForm.ts')
  const dependencies = {
    vue,
    '@/views/accounts/utils/schedulingForm': scheduling,
    '../utils/schedulingForm': scheduling,
    '../utils/modelAccess': load('../src/views/accounts/utils/modelAccess.ts'),
    '@/components/base/BaseToast': { toast },
    '@/api/request': load('../src/api/error.ts'),
    '@/api': { updateAccount: async body => requests.push(structuredClone(body)) },
    '@/api/modules/ipv6-egress': { getIpv6Egress: (options) => {
      reads.push(options)
      return readConfig(options)
    } },
  }
  dependencies['@/composables/useAsyncAction'] = load('../src/composables/useAsyncAction.ts', dependencies)
  const { useAccountEditor } = load('../src/views/accounts/composables/useAccountEditor.ts', dependencies)
  const accounts = vue.ref([{ id: 'acct_test', provider: 'openai', authenticationKind: 'oauth', enabled: true, weight: 1, concurrencyLimit: null, groups: [], requestProxySource: 'account', ...overrides }])
  const scope = vue.effectScope()
  t.after(() => scope.stop())
  const editor = scope.run(() => useAccountEditor({ accounts, reloadAccounts: async () => {}, reloadGroups: async () => {} }))
  return { editor, accounts, requests, reads, errors, scope }
}

test('single account saves egress atomically and cancellation never persists a draft', async (t) => {
  const { editor, accounts, requests } = editorHarness(t, { requestProxySource: 'mihomo', responsesUpstream: 'excel' })
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

test('all saved modes reopen correctly and unchanged or reverted egress is omitted', async (t) => {
  for (const mode of [null, 'unchanged', ...accountIpv6Modes, 'mihomo', 'proxy_pool', 'proxy']) {
    const config = { defaultMode: 'random_ipv6_fresh', accountOverrides: { acct_test: mode } }
    const overrides = mode === 'proxy'
      ? { outboundProxyEndpoint: 'http://proxy.example:8080' }
      : mode === 'mihomo' || mode === 'proxy_pool' ? { requestProxySource: mode } : {}
    const { editor, accounts, requests, errors } = editorHarness(t, overrides, async () => config)
    editor.open(accounts.value[0])
    await settleReads()
    const expected = mode === null ? 'inherit' : mode === 'unchanged' ? 'direct' : accountIpv6Modes.includes(mode) ? 'ipv6' : mode
    assert.equal(editor.proxyMode.value, expected)
    const savedMode = editor.egressMode.value
    editor.proxyMode.value = 'direct'
    editor.proxyMode.value = expected
    editor.weight.value = '7'
    await editor.save()
    assert.equal(requests[0].weight, 7)
    for (const key of ['egressMode', 'outboundProxyId', 'requestProxySource'])
      assert.equal(key in requests[0], false, `${mode}: unchanged save omits ${key}`)
    await vue.nextTick()
    editor.open(accounts.value[0])
    await settleReads()
    assert.equal(editor.proxyMode.value, expected)
    assert.equal(editor.egressMode.value, savedMode)
    assert.deepEqual(errors, [])
  }
})

test('explicit exit changes use the existing patch contract without guessing proxy identity', async (t) => {
  const config = { defaultMode: 'unchanged', accountOverrides: { acct_test: 'random_ipv6_reuse' } }
  const { editor, accounts, requests } = editorHarness(t, {}, async () => config)
  for (const mode of ['inherit', 'direct', 'proxy', 'mihomo', 'proxy_pool', 'ipv6']) {
    editor.open(accounts.value[0])
    await settleReads()
    editor.proxyMode.value = mode
    editor.proxyId.value = 'proxy-selected'
    editor.egressMode.value = 'random_ipv6_fresh'
    await editor.save()
    const expected = plain(accountEgressPatch({ proxyMode: mode, proxyId: 'proxy-selected', egressMode: 'random_ipv6_fresh' }, true))
    for (const key of ['egressMode', 'outboundProxyId', 'requestProxySource'])
      assert.deepEqual(requests.at(-1)[key], expected[key])
    await vue.nextTick()
  }
  config.accountOverrides.acct_test = requests.at(-1).egressMode
  editor.open(accounts.value[0])
  await settleReads()
  assert.equal(editor.proxyMode.value, 'ipv6')
  assert.equal(editor.egressMode.value, 'random_ipv6_fresh')
})

test('failed or pending reads cannot submit defaults but allow saving unrelated settings', async (t) => {
  const result = Promise.withResolvers()
  const { editor, accounts, reads, requests } = editorHarness(t, {}, () => result.promise)
  editor.open(accounts.value[0])
  assert.equal(editor.proxyMode.value, '')
  assert.equal(editor.egressReadState.value.loading, true)
  editor.weight.value = '6'
  await editor.save()
  assert.equal(reads[0].signal.aborted, true)
  for (const key of ['egressMode', 'outboundProxyId', 'requestProxySource'])
    assert.equal(key in requests[0], false)
  result.reject(new Error('read failed'))
  await settleReads()
  assert.equal(editor.egressReadState.value.error, false, 'closed reads have no late effect')

  const failed = editorHarness(t, {}, async () => {
    throw new Error('unavailable')
  })
  failed.editor.open(failed.accounts.value[0])
  await settleReads()
  assert.equal(failed.editor.egressReadState.value.error, true)
  assert.equal(failed.editor.proxyMode.value, '')
  failed.editor.proxyMode.value = 'direct'
  await failed.editor.save()
  assert.equal(failed.requests.length, 0, 'an uninitialized draft cannot overwrite the exit')
  failed.editor.proxyMode.value = ''
  failed.editor.weight.value = '8'
  await failed.editor.save()
  assert.equal(failed.requests[0].weight, 8)
  for (const key of ['egressMode', 'outboundProxyId', 'requestProxySource'])
    assert.equal(key in failed.requests[0], false)
})

test('account switches, close/reopen and disposal abort and fence late reads', async (t) => {
  const results = []
  const { editor, accounts, reads, scope, requests } = editorHarness(t, {}, () => {
    const result = Promise.withResolvers()
    results.push(result)
    return result.promise
  })
  const other = { ...accounts.value[0], id: 'acct_other' }
  accounts.value.push(other)
  editor.open(accounts.value[0])
  editor.open(other)
  assert.equal(reads[0].signal.aborted, true)
  results[0].resolve({ accountOverrides: { acct_test: 'random_ipv6_reuse' } })
  await settleReads()
  assert.equal(editor.proxyMode.value, '')
  results[1].resolve({ accountOverrides: { acct_other: 'fixed_ipv6_fresh' } })
  await settleReads()
  assert.equal(editor.egressMode.value, 'fixed_ipv6_fresh')
  editor.open(other)
  editor.showEditModal.value = false
  editor.open(other)
  assert.equal(reads[2].signal.aborted, true)
  results[2].resolve({ accountOverrides: { acct_other: 'random_ipv6_reuse' } })
  await settleReads()
  assert.equal(editor.proxyMode.value, '')
  results[3].resolve({ accountOverrides: { acct_other: null } })
  await settleReads()
  assert.equal(editor.proxyMode.value, 'inherit')
  editor.open(other)
  scope.stop()
  assert.equal(reads[4].signal.aborted, true)
  results[4].resolve({ accountOverrides: { acct_other: 'fixed_ipv6_reuse' } })
  await settleReads()
  assert.equal(editor.proxyMode.value, '')
  assert.deepEqual(requests, [])
})

test('late supplementary reads and list refreshes never overwrite a user-edited draft', async (t) => {
  const result = Promise.withResolvers()
  const { editor, accounts, requests } = editorHarness(t, { requestProxySource: 'mihomo' }, () => result.promise)
  editor.open(accounts.value[0])
  assert.equal(editor.proxyMode.value, 'mihomo')
  editor.proxyMode.value = 'ipv6'
  editor.egressMode.value = 'random_ipv6_fresh'
  accounts.value[0].requestProxySource = 'proxy_pool'
  result.resolve({ accountOverrides: { acct_test: 'fixed_ipv6_reuse' } })
  await settleReads()
  assert.equal(editor.proxyMode.value, 'ipv6')
  assert.equal(editor.egressMode.value, 'random_ipv6_fresh')
  await editor.save()
  assert.equal(requests[0].egressMode, 'random_ipv6_fresh')
})

test('non-OpenAI accounts do not read IPv6 policy or submit unchanged proxy settings', async (t) => {
  for (const endpoint of [null, 'http://proxy.example:8080']) {
    const { editor, accounts, requests, reads } = editorHarness(t, { provider: 'anthropic', outboundProxyEndpoint: endpoint })
    editor.open(accounts.value[0])
    assert.equal(editor.proxyMode.value, endpoint ? 'proxy' : 'direct')
    await editor.save()
    assert.equal(reads.length, 0)
    for (const key of ['egressMode', 'outboundProxyId', 'requestProxySource'])
      assert.equal(key in requests[0], false)
  }
})
