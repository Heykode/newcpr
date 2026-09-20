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
  runInNewContext(outputText, { exports, TextEncoder, require: name => dependencies[name] ?? require(name) })
  return exports
}
const { templateForm, templateConfig } = load('../src/components/account-templates/template-form.ts', {
  '@/views/accounts/utils/schedulingForm': load('../src/views/accounts/utils/schedulingForm.ts'),
})

test('templates roundtrip scheduling, groups and proxy without sharing mutable arrays', () => {
  const config = { name: 'Team', enabled: false, turnStateInjectionEnabled: true, concurrencyLimit: 8, weight: 19, groupIds: ['group-a'], outboundProxyId: 'proxy-a' }
  const form = templateForm(config)
  assert.equal(JSON.stringify(templateConfig(form)), JSON.stringify(config))
  form.groupIds.push('group-b')
  assert.deepEqual(config.groupIds, ['group-a'])
  const blank = templateForm()
  blank.name = '  Defaults  '
  assert.equal(JSON.stringify(templateConfig(blank)), JSON.stringify({
    name: 'Defaults',
    enabled: true,
    turnStateInjectionEnabled: false,
    concurrencyLimit: null,
    weight: 1,
    groupIds: [],
    outboundProxyId: null,
  }))
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

test('template State is only a boolean and legacy forms become explicit when saved', () => {
  const legacy = templateForm({ name: 'Legacy', turnStateParameters: { model: 'never-copy' } })
  assert.equal(legacy.turnStateInjectionEnabled, false)
  for (const enabled of [true, false]) {
    legacy.turnStateInjectionEnabled = enabled
    const config = templateConfig(legacy)
    assert.equal(config.turnStateInjectionEnabled, enabled)
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
