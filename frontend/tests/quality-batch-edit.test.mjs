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
runInNewContext(outputText, { exports })
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
    excelFailureThreshold: 3,
    ...overrides,
  }
}
const rule = (id, config = base()) => ({ id, revision: 8, config })

test('only checked fields are included and account identity cannot be patched', () => {
  const draft = base({ cron: '0 */12 * * *', model: 'another-model' })
  const patch = buildQualityPatch(['cron'], draft)
  assert.deepEqual(plain(patch), { cron: draft.cron })
  assert.deepEqual(plain(applyQualityPatch(base(), patch)), { ...base(), cron: draft.cron })
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
  await saveQualityBatch(['a', 'b', 'a'], { cron: '15 9 * * *' }, ports)
  assert.deepEqual(calls.map(value => value.id), ['a', 'b'])
  assert.equal(calls[1].revision, 8)
  assert.equal(calls[1].config.judgePrompt, 'fresh-prompt')
  assert.deepEqual(results.map(value => value.success), [true, false])
  const retryIds = results.filter(value => !value.success).map(value => value.id)
  results = []
  fail = false
  current[1] = { ...current[1], revision: 20, config: { ...current[1].config, judgePrompt: 'changed-again' } }
  await saveQualityBatch(retryIds, { cron: '15 9 * * *' }, ports)
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
  await saveQualityBatch(['gone', 'a'], { cron: base().cron }, {
    read: async () => [rule('a')],
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
