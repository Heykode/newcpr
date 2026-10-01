/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

const { outputText } = ts.transpileModule(readFileSync(new URL('../src/views/quality-ops/batch-edit.ts', import.meta.url), 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
})
const exports = {}
const actions = {}
runInNewContext(ts.transpileModule(readFileSync(new URL('../src/views/quality-ops/failure-actions.ts', import.meta.url), 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
}).outputText, { exports: actions })
runInNewContext(outputText, { exports, require: () => actions })
const { applyQualityPatch, buildQualityPatch, saveQualityBatch } = exports
const plain = value => JSON.parse(JSON.stringify(value))
function base(overrides = {}) {
  return {
    detectionMode: 'answer',
    accountId: 'account-a',
    model: 'model-a',
    enabled: true,
    cron: '0 */6 * * *',
    timezone: 'Asia/Shanghai',
    repetitions: 3,
    prompt: 'own question',
    referenceAnswer: 'own answer',
    reasoningEffort: 'high',
    judgeGroupId: 'own-group',
    judgeModel: 'own-judge',
    judgePrompt: 'own instructions',
    failureAction: 'none',
    failureGroupIds: ['group-a'],
    autoRestore: false,
    disableExcelOnNativeRecovery: false,
    excelFailureThreshold: 3,
    ...overrides,
  }
}
const rule = (id, config = base()) => ({ id, revision: 8, config })

test('healthy threshold changes require explicit selection and only apply to native probe recovery actions', () => {
  const probe = base({ detectionMode: 'state_probe', failureAction: 'enable_excel', excelRecoveryThreshold: 2 })
  assert.equal(applyQualityPatch(probe, { model: 'changed' }).excelRecoveryThreshold, 2)
  assert.equal(applyQualityPatch(probe, { excelRecoveryThreshold: 5 }).excelRecoveryThreshold, 5)
  assert.equal(applyQualityPatch(base(), { excelRecoveryThreshold: 5 }).excelRecoveryThreshold, undefined)
  assert.deepEqual(plain(buildQualityPatch(['excelRecoveryThreshold'], probe)), { excelRecoveryThreshold: 2 })
  assert.deepEqual(plain(buildQualityPatch(['model'], probe)), { model: probe.model })
})

test('native recovery is explicitly opted in and never applied to answer or unrelated rules', () => {
  const probe = base({ detectionMode: 'state_probe', failureAction: 'enable_excel' })
  assert.equal(applyQualityPatch(probe, { model: 'changed' }).disableExcelOnNativeRecovery, false)
  const enabled = applyQualityPatch(probe, { disableExcelOnNativeRecovery: true })
  assert.equal(enabled.disableExcelOnNativeRecovery, true)
  assert.equal(enabled.autoRestore, false)
  assert.equal(applyQualityPatch(enabled, { detectionMode: 'answer' }).disableExcelOnNativeRecovery, false)
  assert.equal(applyQualityPatch(enabled, { failureAction: 'none' }).disableExcelOnNativeRecovery, false)
  assert.equal(applyQualityPatch(base(), { disableExcelOnNativeRecovery: true }).disableExcelOnNativeRecovery, false)
  assert.deepEqual(plain(buildQualityPatch(['disableExcelOnNativeRecovery'], enabled)), { disableExcelOnNativeRecovery: true })
})

test('only checked fields are included and account identity cannot be patched', () => {
  const draft = base({ intervalSeconds: 75, model: 'another-model' })
  const patch = buildQualityPatch(['intervalSeconds'], draft)
  assert.deepEqual(plain(patch), { intervalSeconds: 75 })
  assert.deepEqual(plain(applyQualityPatch(base(), patch)), { ...base(), intervalSeconds: 75 })
  assert.equal(applyQualityPatch(base(), { model: 'changed' }).intervalSeconds, undefined)
  assert.equal(applyQualityPatch(base({ intervalSeconds: 90 }), { model: 'changed' }).intervalSeconds, 90)
  assert.equal(applyQualityPatch(base(), { accountId: 'must-not-change' }).accountId, 'account-a')
  assert.deepEqual(plain(buildQualityPatch([], draft)), {})
})

test('per-rule configuration and independent array values are preserved', () => {
  const source = base({ failureAction: 'remove_groups' })
  const patch = buildQualityPatch(['failureGroupIds'], source)
  patch.failureGroupIds.push('group-b')
  assert.deepEqual(source.failureGroupIds, ['group-a'])
  const next = applyQualityPatch(source, patch)
  next.failureGroupIds.push('group-c')
  assert.deepEqual(plain(patch.failureGroupIds), ['group-a', 'group-b'])
  assert.equal(next.judgePrompt, source.judgePrompt)
  assert.equal(next.excelFailureThreshold, 3)
})

test('answer-only edits do not change probe rules or erase their question drafts', () => {
  const source = base({ detectionMode: 'state_probe', repetitions: 1, reasoningEffort: null })
  const next = applyQualityPatch(source, { prompt: 'replacement', repetitions: 5, judgeModel: 'replacement' })
  assert.deepEqual(plain(next), source)
  const converted = applyQualityPatch(base(), { detectionMode: 'state_probe', repetitions: 5 })
  assert.equal(converted.repetitions, 1)
  assert.equal(converted.reasoningEffort, null)
  assert.equal(converted.prompt, 'own question')
})

test('Excel threshold applies only to Excel actions and never enables automatic closing', () => {
  assert.equal(applyQualityPatch(base(), { excelFailureThreshold: 8 }).excelFailureThreshold, 3)
  const next = applyQualityPatch(base({ autoRestore: true }), { failureAction: 'enable_excel', excelFailureThreshold: 8 })
  assert.equal(next.excelFailureThreshold, 8)
  assert.equal(next.autoRestore, false)
  assert.equal(applyQualityPatch(next, { autoRestore: true }).autoRestore, false)
})

test('template action reuses threshold and preserves each unselected template', () => {
  const template = { id: 'template-a', revision: 3, config: { name: 'Excel', responsesUpstream: 'excel', egressMode: 'random_ipv6_reuse' } }
  const source = base({ failureAction: 'apply_account_template', failureTemplate: template })
  const next = applyQualityPatch(source, { excelFailureThreshold: 6, autoRestore: true })
  assert.deepEqual(plain(next.failureTemplate), template)
  assert.equal(next.excelFailureThreshold, 6)
  assert.equal(next.autoRestore, false)
  assert.throws(() => applyQualityPatch(base(), { failureAction: 'apply_account_template' }), /请选择/)
  assert.equal('failureTemplate' in applyQualityPatch(source, { failureAction: 'none' }), false)
  const replacement = { ...template, revision: 4 }
  assert.equal(applyQualityPatch(source, { failureTemplate: replacement }).failureTemplate.revision, 4)
  assert.equal('failureTemplate' in applyQualityPatch(base(), { failureTemplate: replacement }), false)
  assert.deepEqual(plain(buildQualityPatch(['failureTemplate'], source)), { failureTemplate: template })
})

test('new rules choose templates instead of standalone Excel while legacy rules remain editable', () => {
  assert.equal(actions.failureActionOptions('none').some(value => value.value === 'enable_excel'), false)
  assert.equal(actions.failureActionOptions('none').some(value => value.value === 'apply_account_template'), true)
  assert.equal(actions.failureActionOptions('enable_excel').some(value => value.value === 'enable_excel'), true)
})

test('batch refreshes revisions, preserves fresh unrelated values and retries failures only', async () => {
  const calls = []
  let current = [rule('a'), rule('b', base({ model: 'fresh-model', judgePrompt: 'fresh-prompt' }))]
  let fail = true
  let results = []
  const ports = {
    read: async () => current,
    save: async (request) => {
      calls.push(plain(request))
      if (request.id === 'b' && fail)
        throw new Error('revision conflict')
      const next = { ...request, revision: request.revision + 1 }
      current = current.map(value => value.id === request.id ? next : value)
      return next
    },
    stopped: () => false,
    onResult: result => results.push(plain(result)),
  }
  await saveQualityBatch(['a', 'b', 'a'], { intervalSeconds: 75 }, ports)
  assert.deepEqual(calls.map(value => value.id), ['a', 'b'])
  assert.equal(calls[1].revision, 8)
  assert.equal(calls[1].config.judgePrompt, 'fresh-prompt')
  assert.deepEqual(results.map(value => value.success), [true, false])
  const retryIds = results.filter(value => !value.success).map(value => value.id)
  results = []
  fail = false
  current[1] = { ...current[1], revision: 20, config: { ...current[1].config, judgePrompt: 'changed-again' } }
  await saveQualityBatch(retryIds, { intervalSeconds: 75 }, ports)
  assert.deepEqual(calls.map(value => value.id), ['a', 'b', 'b'])
  assert.equal(calls[2].revision, 20)
  assert.equal(calls[2].config.judgePrompt, 'changed-again')
})

test('failed refresh and empty selection do not write any rule', async () => {
  let writes = 0
  const ports = {
    read: async () => { throw new Error('refresh unavailable') },
    save: async () => { writes++ },
    stopped: () => false,
    onResult: () => {},
  }
  await assert.rejects(saveQualityBatch(['a'], { enabled: false }, ports), /refresh unavailable/)
  await assert.rejects(saveQualityBatch(['a'], {}, ports), /请先勾选/)
  assert.equal(writes, 0)
})

test('deleted rules fail explicitly and unchanged rules are not saved', async () => {
  let writes = 0
  const results = []
  await saveQualityBatch(['gone', 'a'], { intervalSeconds: 60 }, {
    read: async () => [rule('a', base({ intervalSeconds: 60 }))],
    save: async () => { writes++ },
    stopped: () => false,
    onResult: value => results.push(plain(value)),
  })
  assert.equal(writes, 0)
  assert.equal(results[0].success, false)
  assert.equal(results[1].success, true)
  assert.equal(results[1].message, '无需修改')
})

test('stop or unmount prevents subsequent requests after the in-flight save', async () => {
  let stopped = false
  let writes = 0
  await saveQualityBatch(['a', 'b'], { enabled: false }, {
    read: async () => [rule('a'), rule('b')],
    save: async (request) => {
      writes++
      stopped = true
      return request
    },
    stopped: () => stopped,
    onResult: () => {},
  })
  assert.equal(writes, 1)
})
