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
const { templateForm, templateConfig } = load('../src/views/relogin/template-form.ts', {
  '../accounts/utils/schedulingForm': load('../src/views/accounts/utils/schedulingForm.ts'),
})

test('templates roundtrip scheduling, groups and proxy without sharing mutable arrays', () => {
  const config = { name: 'Team', enabled: false, concurrencyLimit: 8, weight: 19, groupIds: ['group-a'], outboundProxyId: 'proxy-a' }
  const form = templateForm(config)
  assert.equal(JSON.stringify(templateConfig(form)), JSON.stringify(config))
  form.groupIds.push('group-b')
  assert.deepEqual(config.groupIds, ['group-a'])
  const blank = templateForm()
  blank.name = '  Defaults  '
  assert.equal(JSON.stringify(templateConfig(blank)), JSON.stringify({
    name: 'Defaults',
    enabled: true,
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
  await api.saveReloginTemplate({ name: 'Test' }, { id: 'template-a', revision: 3 })
  assert.equal(calls[2].url, '/api/admin/relogin/templates/save')
  assert.equal(calls[2].data.selection.revision, 3)
  await api.deleteReloginTemplate({ id: 'template-a', revision: 4 })
  assert.equal(calls[3].data.revision, 4)
})
