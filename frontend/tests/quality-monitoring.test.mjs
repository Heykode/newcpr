/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

const { outputText } = ts.transpileModule(readFileSync(new URL('../src/components/quality-ops/monitoring.ts', import.meta.url), 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
})
const exports = {}
runInNewContext(outputText, { exports })
const { monitoringPresentation, qualityTemplateTargets } = exports

test('monitoring labels distinguish scheduled, queued, running and disabled rules', () => {
  const idle = { enabled: true, running: false, pending: false, lastStatus: 'incorrect' }
  assert.equal(monitoringPresentation(idle).label, '监测中')
  assert.equal(monitoringPresentation({ ...idle, pending: true }).label, '已排队')
  assert.equal(monitoringPresentation({ ...idle, running: true, pending: true }).label, '检测中')
  assert.equal(monitoringPresentation({ ...idle, enabled: false, running: true }).label, '监测暂停')
})

test('application captures exact rule versions across page boundaries without mutating selection', () => {
  const ids = ['on-page', 'off-page', 'new']
  const state = {
    'on-page': { ruleId: 'rule-a', revision: 4 },
    'off-page': { ruleId: 'rule-b', revision: 8 },
  }
  assert.equal(JSON.stringify(qualityTemplateTargets(ids, state)), JSON.stringify([
    { accountId: 'on-page', ruleId: 'rule-a', revision: 4 },
    { accountId: 'off-page', ruleId: 'rule-b', revision: 8 },
    { accountId: 'new', ruleId: null, revision: null },
  ]))
  assert.deepEqual(ids, ['on-page', 'off-page', 'new'])
})
