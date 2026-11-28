/* eslint-disable test/no-import-node-test -- uses the project's Node test runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import { createTestRequire as createRequire } from './helpers/i18n.mjs'

const require = createRequire(import.meta.url)
function load(path, dependencies = {}) {
  if (path !== '../src/utils/account-egress.ts')
    dependencies['@/utils/account-egress'] = load('../src/utils/account-egress.ts')
  const source = readFileSync(new URL(path, import.meta.url), 'utf8')
  const { outputText } = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  const exports = {}
  runInNewContext(outputText, { exports, TextEncoder, require: name => dependencies[name] ?? (require(name)) })
  return exports
}
const { templateForm, templateConfig } = load('../src/components/account-templates/template-form.ts', {
  '@/views/accounts/utils/modelAccess': load('../src/views/accounts/utils/modelAccess.ts'),
  '@/views/accounts/utils/schedulingForm': load('../src/views/accounts/utils/schedulingForm.ts'),
})

test('template policies preserve omission and null but roundtrip explicit model restrictions', () => {
  for (const value of [undefined, null]) {
    const form = templateForm({ name: 'Legacy policy', modelAccess: value })
    assert.equal(form.modelAccess, undefined)
    assert.equal('modelAccess' in templateConfig(form), false)
  }
  assert.equal(templateForm().modelAccess, undefined)
  for (const mode of ['all', 'allowlist', 'denylist']) {
    const policy = { mode, models: mode === 'all' ? [] : ['model-a', 'model-b'] }
    const expected = JSON.stringify(policy)
    const stored = { name: 'Model policy', modelAccess: policy }
    const form = templateForm(stored)
    const saved = templateConfig(form)
    assert.equal(JSON.stringify(saved.modelAccess), expected)
    form.modelAccess.models.push('model-c')
    assert.equal(JSON.stringify(stored.modelAccess), expected)
    assert.equal(JSON.stringify(saved.modelAccess), expected)
    const reopened = templateForm(saved)
    assert.equal(JSON.stringify(reopened.modelAccess), expected)
    reopened.modelAccess = undefined
    assert.equal('modelAccess' in templateConfig(reopened), false)
    assert.equal(JSON.stringify(saved.modelAccess), expected)
  }
})

test('template model restrictions reuse exact ID and nonempty-list validation', () => {
  for (const mode of ['allowlist', 'denylist']) {
    for (const models of [[], ['model-*'], ['__reserved'], ['bad\nmodel'], [' model-a '], ['x'.repeat(257)], Array.from({ length: 257 }, (_, i) => `model-${i}`)]) {
      const form = { ...templateForm(), name: 'Invalid policy', modelAccess: { mode, models } }
      assert.throws(() => templateConfig(form))
    }
  }
  const form = { ...templateForm(), name: 'Remove restrictions', modelAccess: { mode: 'all', models: [] } }
  assert.equal(JSON.stringify(templateConfig(form).modelAccess), '{"mode":"all","models":[]}')
})

test('managed exit templates do not require or toggle Excel', () => {
  const form = templateForm({ name: 'Native proxy', requestProxySource: 'mihomo' })
  assert.equal(form.proxyMode, 'mihomo')
  const config = templateConfig(form)
  assert.equal(config.requestProxySource, 'mihomo')
  assert.equal('responsesUpstream' in config, false)
  assert.equal('excelModels' in config, false)
  form.proxyMode = 'preserve'
  assert.equal('requestProxySource' in templateConfig(form), false)
  assert.equal(templateConfig(form).preserveOutboundProxy, true)
})

test('templates roundtrip scheduling, groups and proxy without sharing mutable arrays', () => {
  const config = { name: 'Team', enabled: false, concurrencyLimit: 8, weight: 19, groupIds: ['group-a'], outboundProxyId: 'proxy-a' }
  const form = templateForm(config)
  assert.equal(JSON.stringify(templateConfig(form)), JSON.stringify(config))
  form.groupIds.push('group-b')
  assert.deepEqual(config.groupIds, ['group-a'])
  const blank = templateForm()
  blank.name = '  Defaults  '
  assert.deepEqual(JSON.parse(JSON.stringify(templateConfig(blank))), {
    name: 'Defaults',
    enabled: true,
    concurrencyLimit: null,
    weight: 1,
    groupIds: [],
    preserveOutboundProxy: true,
    outboundProxyId: null,
  })
})

test('IPv6 templates distinguish preserve, inherit and all existing modes', () => {
  const legacy = { name: 'Legacy', enabled: true, concurrencyLimit: null, weight: 1, groupIds: [], outboundProxyId: null }
  assert.equal('egressMode' in templateConfig(templateForm(legacy)), false)
  for (const mode of [null, 'unchanged', 'fixed_ipv6_reuse', 'random_ipv6_reuse', 'fixed_ipv6_fresh', 'random_ipv6_fresh']) {
    const config = { ...legacy, egressMode: mode }
    const form = templateForm(config)
    assert.equal(templateConfig(form).egressMode, mode)
    form.proxyMode = 'preserve'
    assert.equal('egressMode' in templateConfig(form), false)
    assert.equal(templateConfig(form).preserveOutboundProxy, true)
  }
  const proxy = { ...templateForm(legacy), proxyMode: 'proxy', proxyId: 'proxy-a' }
  assert.equal(templateConfig({ ...proxy, egressMode: 'fixed_ipv6_reuse' }).egressMode, 'unchanged')
  const inherit = templateConfig({ ...templateForm(), name: 'Global', proxyMode: 'inherit' })
  assert.equal(inherit.egressMode, null)
  assert.equal(inherit.requestProxySource, 'account')
  assert.equal(inherit.preserveOutboundProxy, false)
  assert.equal(inherit.outboundProxyId, null)
})

test('template validation reuses account scheduling limits and rejects missing proxy and invalid names', () => {
  for (const values of [
    { name: '' },
    { name: 'x\nx' },
    { name: '模'.repeat(43) },
    { concurrencyLimit: '0' },
    { concurrencyLimit: '1.5' },
    { concurrencyLimit: '4294967296' },
    { weight: '101' },
    { proxyMode: 'proxy', proxyId: '' },
  ]) {
    assert.throws(() => templateConfig({ ...templateForm(), name: 'Test', ...values }))
  }
})

test('push snapshots template revision and remains backward compatible without one', async () => {
  const calls = []
  const api = load('../src/api/modules/relogin.ts', {
    '../request': async request => calls.push(request),
  })
  await api.pushRelogin([{ id: 'new', revision: 7 }], { id: 'template-a', revision: 3 })
  assert.equal(JSON.stringify(calls[0].data), JSON.stringify({
    ids: ['new'],
    revisions: { new: 7 },
    template: { id: 'template-a', revision: 3 },
  }))
  await api.pushRelogin([{ id: 'old', revision: 9 }])
  assert.equal('template' in calls[1].data, false)
  const templates = load('../src/api/modules/account-templates.ts', {
    '../request': async request => calls.push(request),
  })
  await templates.saveAccountTemplate({ name: 'Test' }, { id: 'template-a', revision: 3 })
  assert.equal(calls[2].url, '/api/admin/relogin/templates/save')
  assert.equal(calls[2].data.selection.revision, 3)
  await templates.deleteAccountTemplate({ id: 'template-a', revision: 4 })
  assert.equal(calls[3].data.revision, 4)
})

test('legacy template State fields are ignored and never written back', () => {
  const legacy = templateForm({ name: 'Legacy', turnStateParameters: { model: 'never-copy' } })
  assert.equal('turnStateInjectionEnabled' in legacy, false)
  for (const enabled of [true, false]) {
    legacy.turnStateInjectionEnabled = enabled
    const config = templateConfig(legacy)
    assert.equal('turnStateInjectionEnabled' in config, false)
    assert.equal('turnStateParameters' in config, false)
  }
})

test('account application captures IDs and version without resubmitting template config', async () => {
  let call
  const api = load('../src/api/modules/account-templates.ts', {
    '../request': async (request) => { call = request },
  })
  const ids = ['first', 'second']
  const template = { id: 'template', revision: 5, config: { enabled: false } }
  await api.applyAccountTemplate(ids, template)
  ids.push('later')
  template.revision = 6
  assert.equal(call.url, '/api/admin/accounts/apply-template')
  assert.equal(JSON.stringify(call.data), JSON.stringify({
    accountIds: ['first', 'second'],
    template: { id: 'template', revision: 5 },
  }))
})

test('templates never carry account names and push names are independent of templates', async () => {
  const form = { ...templateForm(), name: 'Common settings', customName: 'Never apply' }
  assert.equal('customName' in templateConfig(form), false)
  assert.equal('customName' in templateForm({ ...templateConfig(form), customName: 'Never copy' }), false)
  const calls = []
  const api = load('../src/api/modules/relogin.ts', {
    '../request': async request => calls.push(request),
  })
  await api.pushRelogin([{ id: 'new', revision: 1 }], undefined, 'Batch A')
  assert.equal(calls[0].data.customName, 'Batch A')
  assert.equal('template' in calls[0].data, false)
  await api.pushRelogin([{ id: 'new', revision: 1 }], { id: 'template', revision: 2 }, 'Batch B')
  assert.equal(calls[1].data.customName, 'Batch B')
  assert.equal('customName' in calls[1].data.template, false)
  await api.pushRelogin([{ id: 'old', revision: 1 }])
  assert.equal('customName' in calls[2].data, false)
})

test('credential maintenance template choice preserves keep, follow and separate proxy', () => {
  for (const choice of [undefined, null, 'maintenance-proxy']) {
    const form = { ...templateForm(), name: 'Maintenance defaults', credentialProxyId: choice }
    const config = templateConfig(form)
    assert.equal(Object.hasOwn(config, 'credentialProxyId'), choice !== undefined)
    assert.equal(config.credentialProxyId, choice)
    assert.equal(templateForm(config).credentialProxyId, choice)
    assert.equal(config.outboundProxyId, null)
  }
})

test('retired template fields cannot survive editing and ordinary settings stay intact', () => {
  const config = { name: 'Saved template', enabled: true, concurrencyLimit: 3, weight: 10, groupIds: ['group-a'], preserveOutboundProxy: true, outboundProxyId: null, requestProxySource: 'account', responsesUpstream: 'excel', excelModels: ['legacy'], excelRecovery: { enabled: true, intervalMinutes: 5 }, excelIgnoreEncryptedContent: true }
  const saved = templateConfig(templateForm(config))
  assert.equal(Object.keys(saved).some(key => key.startsWith('excel') || key === 'responsesUpstream'), false)
  for (const key of ['name', 'enabled', 'concurrencyLimit', 'weight', 'groupIds', 'preserveOutboundProxy', 'outboundProxyId', 'requestProxySource'])
    assert.deepEqual(JSON.parse(JSON.stringify(saved[key])), config[key])
})
