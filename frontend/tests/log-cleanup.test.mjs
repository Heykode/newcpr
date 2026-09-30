/* eslint-disable test/no-import-node-test -- Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

const { outputText } = ts.transpileModule(readFileSync(new URL('../src/views/settings/components/cleanup/cleanup.ts', import.meta.url), 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
})
const exports = {}
runInNewContext(outputText, { exports, Date, Number, Math })
const { cleanupBytes, cleanupCutoff, cleanupValidation, cleanupCategories } = exports
test('occupancy distinguishes failed measurement from real zero and uses decimal GB', () => {
  assert.equal(cleanupBytes(null), '读取失败')
  assert.equal(cleanupBytes(undefined), '读取失败')
  assert.equal(cleanupBytes(Number.NaN), '读取失败')
  assert.equal(cleanupBytes(-1), '读取失败')
  assert.equal(cleanupBytes(0), '0 B')
  assert.equal(cleanupBytes(27320000000), '27.32 GB')
})
test('cleanup confirmation uses a frozen cutoff, not the current browser clock', () => {
  assert.equal(cleanupCutoff('2026-09-30T00:00:00Z', 31), '2026-08-30T00:00:00.000Z')
})
test('retention validation preserves request history and capture limits', () => {
  const config = Object.fromEntries(cleanupCategories.map(({ key, min }) => [key, { selected: true, retentionDays: min }]))
  assert.equal(cleanupValidation(config), '')
  config.requests.retentionDays = 30
  assert.notEqual(cleanupValidation(config), '')
  config.requests.retentionDays = 31
  config.captures.retentionDays = 31
  assert.notEqual(cleanupValidation(config), '')
})
