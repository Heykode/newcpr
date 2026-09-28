/* eslint-disable test/no-import-node-test -- uses the project's Node test runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

const require = createRequire(import.meta.url)
function load(path, dependencies = {}) {
  const source = readFileSync(new URL(path, import.meta.url), 'utf8')
  const { outputText } = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  const exports = {}
  runInNewContext(outputText, { exports, TextEncoder, require: name => dependencies[name] ?? (name === '@/utils/excel-defaults' ? load('../src/utils/excel-defaults.ts') : require(name)) })
  return exports
}
const { templateForm, templateConfig } = load('../src/components/account-templates/template-form.ts', {
  '@/utils/excel-settings': load('../src/utils/excel-settings.ts', {
    '@/views/accounts/utils/schedulingForm': load('../src/views/accounts/utils/schedulingForm.ts'),
  }),
  '@/views/accounts/utils/schedulingForm': load('../src/views/accounts/utils/schedulingForm.ts'),
})

test('managed exit templates do not require or toggle Excel', () => {
  const form = templateForm({ name: 'Native proxy', requestProxySource: 'mihomo' })
  assert.equal(form.applyExcel, false)
  assert.equal(form.applyRequestProxySource, true)
  const config = templateConfig(form)
  assert.equal(config.requestProxySource, 'mihomo')
  assert.equal('responsesUpstream' in config, false)
  assert.equal('excelModels' in config, false)
  form.applyRequestProxySource = false
  assert.equal('requestProxySource' in templateConfig(form), false)
})

test('templates roundtrip scheduling, groups and proxy without sharing mutable arrays', () => {
  const config = { name: 'Team', enabled: false, concurrencyLimit: 8, weight: 19, groupIds: ['group-a'], outboundProxyId: 'proxy-a' }
  const form = templateForm(config)
  assert.equal(JSON.stringify(templateConfig(form)), JSON.stringify(config))
  form.groupIds.push('group-b')
  assert.deepEqual(config.groupIds, ['group-a'])
  const blank = templateForm()
  assert.equal(blank.excelModels, 'gpt-6-astra, gpt-5.6-sol, gpt-5.6-terra')
  blank.name = '  Defaults  '
  assert.deepEqual(JSON.parse(JSON.stringify(templateConfig(blank))), {
    name: 'Defaults',
    enabled: true,
    responsesUpstream: 'codex',
    excelModelsFollowGlobal: true,
    excelCacheCreationAsInput: true,
    excelIgnoreEncryptedContent: false,
    excel403Action: 'none',
    concurrencyLimit: null,
    weight: 1,
    groupIds: [],
    outboundProxyId: null,
  })
})

test('IPv6 templates distinguish preserve, inherit and all existing modes', () => {
  const legacy = { name: 'Legacy', enabled: true, concurrencyLimit: null, weight: 1, groupIds: [], outboundProxyId: null }
  assert.equal(templateForm(legacy).egressMode, 'preserve')
  assert.equal('egressMode' in templateConfig(templateForm(legacy)), false)
  for (const mode of [null, 'unchanged', 'fixed_ipv6_reuse', 'random_ipv6_reuse', 'fixed_ipv6_fresh', 'random_ipv6_fresh']) {
    const config = { ...legacy, egressMode: mode }
    const form = templateForm(config)
    assert.equal(form.egressMode, mode ?? 'inherit')
    assert.equal(templateConfig(form).egressMode, mode)
    form.egressMode = 'preserve'
    assert.equal('egressMode' in templateConfig(form), false)
  }
  const proxy = { ...templateForm(legacy), proxyMode: 'proxy', proxyId: 'proxy-a' }
  assert.throws(() => templateConfig({ ...proxy, egressMode: 'fixed_ipv6_reuse' }), /IPv6/)
  for (const egressMode of ['preserve', 'inherit', 'unchanged'])
    assert.doesNotThrow(() => templateConfig({ ...proxy, egressMode }))
})

test('Excel templates preserve legacy omission and roundtrip global/custom/empty lists', () => {
  const legacy = templateConfig(templateForm({ name: 'Legacy' }))
  assert.equal('responsesUpstream' in legacy, false)
  assert.equal('excelModelsFollowGlobal' in legacy, false)
  const form = templateForm()
  form.name = 'Excel'
  form.excelEnabled = true
  assert.equal(templateConfig(form).responsesUpstream, 'excel')
  assert.equal(templateConfig(form).excelModelsFollowGlobal, true)
  assert.equal('excelModels' in templateConfig(form), false)
  assert.equal(templateConfig(form).excelCacheCreationAsInput, true)
  form.excelCacheCreationAsInput = true
  assert.equal(templateConfig(form).excelCacheCreationAsInput, true)
  assert.equal(templateConfig(form).excel403Action, 'none')
  form.excel403Action = 'pause_account'
  assert.equal(templateConfig(form).excel403Action, 'pause_account')
  assert.equal(templateForm(templateConfig(form)).excel403Action, 'pause_account')
  form.excelEnabled = false
  assert.equal(templateConfig(form).excel403Action, 'none')
  assert.equal(templateConfig(form).excelCacheCreationAsInput, true)
  form.excelEnabled = true
  form.excelModelsFollowGlobal = false
  form.excelModels = 'gpt-6-astra, gpt-6-astra'
  assert.equal(JSON.stringify(templateConfig(form).excelModels), '["gpt-6-astra"]')
  form.excelModels = ''
  assert.equal(JSON.stringify(templateConfig(form).excelModels), '[]')
  form.excelModels = 'invalid/model'
  assert.throws(() => templateConfig(form))
  form.excelModelsFollowGlobal = true
  assert.doesNotThrow(() => templateConfig(form))
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

test('encrypted history omission is opt-in, roundtrips and is absent without Excel settings', () => {
  const form = templateForm()
  form.name = 'Encrypted history'
  form.excelEnabled = true
  assert.equal(form.excelIgnoreEncryptedContent, false)
  for (const enabled of [true, false]) {
    form.excelIgnoreEncryptedContent = enabled
    assert.equal(templateConfig(form).excelIgnoreEncryptedContent, enabled)
    assert.equal(templateForm(templateConfig(form)).excelIgnoreEncryptedContent, enabled)
  }
  form.excelIgnoreEncryptedContent = true
  form.excelEnabled = false
  assert.equal(templateConfig(form).excelIgnoreEncryptedContent, false)
  form.applyExcel = false
  assert.equal('excelIgnoreEncryptedContent' in templateConfig(form), false)
  assert.equal(templateForm({ name: 'Legacy' }).excelIgnoreEncryptedContent, false)
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
