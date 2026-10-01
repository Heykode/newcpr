/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

function load(name) {
  const exports = {}
  const { outputText } = ts.transpileModule(readFileSync(new URL(`../src/views/quality-ops/${name}.ts`, import.meta.url), 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, { exports, require: name => load(name.replace('./', '')) })
  return exports
}
const { newQualityConfig, qualityConfigDraft } = load('defaults')
const plain = value => JSON.parse(JSON.stringify(value))

test('new rules and templates share reference probe defaults without applying a remediation template', () => {
  const config = newQualityConfig('Asia/Shanghai')
  assert.equal(config.detectionMode, 'state_probe')
  assert.equal(config.model, 'gpt-6-astra')
  assert.equal(config.intervalSeconds, 120)
  assert.equal(config.repetitions, 1)
  assert.equal(config.excelFailureThreshold, 2)
  assert.equal(config.excelRecoveryThreshold, 2)
  assert.equal(config.disableExcelOnNativeRecovery, true)
  assert.equal(config.failureAction, 'none')
  assert.equal(config.failureTemplate, null)
  config.failureGroupIds.push('fixture')
  assert.deepEqual(plain(newQualityConfig('UTC').failureGroupIds), [])
})

test('editing legacy rules or templates never adopts new mode schedule or recovery behavior', () => {
  const legacy = { model: 'old-model', cron: '17 8 * * *', timezone: 'UTC', failureGroupIds: ['old-group'] }
  const draft = qualityConfigDraft(legacy, 'Asia/Shanghai')
  assert.equal(draft.detectionMode, 'answer')
  assert.equal(draft.model, 'old-model')
  assert.equal(draft.intervalSeconds, null)
  assert.equal(draft.cron, legacy.cron)
  assert.equal(draft.timezone, 'UTC')
  assert.equal(draft.disableExcelOnNativeRecovery, false)
  assert.equal(draft.excelFailureThreshold, 1)
  assert.equal(draft.excelRecoveryThreshold, 1)
  draft.failureGroupIds.push('new-group')
  assert.deepEqual(legacy.failureGroupIds, ['old-group'])
})

test('explicit saved thresholds and custom schedules survive editing and template reuse', () => {
  const config = { ...newQualityConfig('UTC'), model: 'custom', intervalSeconds: 17, excelFailureThreshold: 7, excelRecoveryThreshold: 4, disableExcelOnNativeRecovery: false }
  assert.deepEqual(plain(qualityConfigDraft(config, 'Asia/Shanghai')), plain(config))
  const oldProbe = { ...config }
  delete oldProbe.excelRecoveryThreshold
  assert.equal(qualityConfigDraft(oldProbe, 'UTC').excelRecoveryThreshold, 1)
})
