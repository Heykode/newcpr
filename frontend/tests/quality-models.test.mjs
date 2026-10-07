/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

const exports = {}
const { outputText } = ts.transpileModule(readFileSync(new URL('../src/views/quality-ops/model-choices.ts', import.meta.url), 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
})
runInNewContext(outputText, { exports })
const { mergeModelChoices, qualityEffortOptions } = exports
const plain = value => JSON.parse(JSON.stringify(value))

test('model suggestions merge pages without changing manual values or inventing capabilities', () => {
  const first = [{ id: 'one', name: 'One', reasoningEfforts: null }]
  const choices = mergeModelChoices(first, [{ id: 'two', name: 'Two', reasoningEfforts: ['high', 'max'] }])
  assert.equal(choices.length, 2)
  assert.deepEqual(first, [{ id: 'one', name: 'One', reasoningEfforts: null }])
  assert.equal(qualityEffortOptions(choices, 'manual-model', 'legacy-effort').at(-1).value, 'legacy-effort')
  assert.ok(qualityEffortOptions(choices, 'one', '').slice(1).every(option => option.label.endsWith('（自定义）')))
  assert.deepEqual(plain(qualityEffortOptions(choices, 'two', '').filter(option => option.label.endsWith('（目录支持）')).map(option => option.value)), ['high', 'max'])
  assert.equal(qualityEffortOptions(choices, 'two', '')[0].value, '', 'catalog never injects a default effort')
})
