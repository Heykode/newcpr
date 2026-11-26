/* eslint-disable test/no-import-node-test -- regression uses Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import { createTestRequire } from './helpers/i18n.mjs'

const require = createTestRequire(import.meta.url)
const filename = new URL('../src/views/accounts/constants.ts', import.meta.url)
const { outputText } = ts.transpileModule(readFileSync(filename, 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
})
const numberExports = {}
const numberCode = ts.transpileModule(readFileSync(new URL('../src/utils/number.ts', import.meta.url), 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
}).outputText
runInNewContext(numberCode, { exports: numberExports, require })
const exports = {}
runInNewContext(outputText, {
  exports,
  require: name => name === '@/components/base/BaseTable/columns'
    ? { defineTableColumns: columns => columns }
    : name === '@/utils/number' ? numberExports : require(name),
})

test('account filters expose the fifteen approved Codex plans even with no account rows', () => {
  const options = JSON.parse(JSON.stringify(exports.codexPlanFilterOptions))
  assert.deepEqual(options.map(option => option.label), [
    '全部套餐',
    'Free',
    'Go',
    'Plus',
    'ProLite',
    'Pro',
    'ProMax',
    'Business',
    'Business Premium',
    'Enterprise',
    'Enterprise (Automation)',
    'Edu',
    'Edu Plus',
    'Edu Pro',
    'Academic Researchers',
    'Academic Researchers Plus',
  ])
  assert.equal(options[0].value, '')
  assert.equal(new Set(options.map(option => option.value)).size, 16)
  assert.ok(options.slice(1).every(option => option.value.startsWith('openai:')))
  assert.equal(options.find(option => option.label === 'Business').value, 'openai:team')
  assert.equal(options.find(option => option.label === 'Business Premium').value, 'openai:self_serve_business_prolite')
  assert.equal(options.find(option => option.label === 'Enterprise').value, 'openai:enterprise')
})

test('Antigravity quota names are localized while unknown names and window periods remain visible', () => {
  const window = { key: 'model-window', limitId: 'gemini', limitName: 'Gemini Models', windowSeconds: 604800, role: null, windowLabelDisplay: 'fallback' }
  assert.equal(exports.quotaLimitLabel(window), 'Gemini 模型')
  assert.equal(exports.quotaWindowLabel(window), 'Gemini 模型 · 周额度')
  assert.equal(exports.quotaWindowLabel(window, false), '周额度')
  assert.equal(exports.quotaWindowLabel({ ...window, limitName: 'Future Models', windowSeconds: null }), 'Future Models · fallback')
})
