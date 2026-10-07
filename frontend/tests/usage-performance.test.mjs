/* eslint-disable test/no-import-node-test -- Node's built-in runner matches existing tests. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

const { outputText } = ts.transpileModule(readFileSync(new URL('../src/views/usage/utils/records.ts', import.meta.url), 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
})
const module = { exports: {} }
runInNewContext(outputText, {
  exports: module.exports,
  require: name => ({
    '@/utils/object': { isRecord: value => value !== null && typeof value === 'object' && !Array.isArray(value) },
    './format': { formatDuration: value => value === null ? 'missing' : `${value}ms` },
  })[name],
})
const { usagePerformanceDetails, usageLatencyDetails } = module.exports
function record(overrides = {}) {
  return {
    latencyMs: 3500,
    firstTokenLatencyMs: 1000,
    latencyDetails: { firstTokenMs: 500, firstEventMs: 100 },
    tokenDetails: { outputTokens: 125 },
    ...overrides,
  }
}

test('throughput measures output after request-relative TTFT, including prior attempt overhead', () => {
  const result = usagePerformanceDetails(record())
  assert.equal(result.throughputDisplay, '50 tok/s')
  assert.equal(result.firstTokenDisplay, '1000ms')
  assert.equal(usageLatencyDetails(record()).firstOutputDisplay, '1000ms')
  assert.equal(usagePerformanceDetails(record({ firstTokenLatencyMs: null })).throughputDisplay, '41.7 tok/s')
  assert.equal(usagePerformanceDetails(record({ firstTokenLatencyMs: 0 })).throughputDisplay, '35.7 tok/s')
})

test('missing or invalid measurements never invent throughput or substitute the first lifecycle event', () => {
  for (const overrides of [
    { tokenDetails: null },
    { tokenDetails: { outputTokens: 0 } },
    { tokenDetails: { outputTokens: -1 } },
    { tokenDetails: { outputTokens: Number.NaN } },
    { tokenDetails: { outputTokens: Number.POSITIVE_INFINITY } },
    { latencyMs: 1000 },
    { latencyMs: 999 },
    { latencyMs: null },
    { firstTokenLatencyMs: -1 },
    { firstTokenLatencyMs: null, latencyDetails: { firstEventMs: 100 } },
  ]) {
    const value = usagePerformanceDetails(record(overrides)).throughputDisplay
    assert.equal(value, '\u2014')
    assert.doesNotMatch(value, /NaN|Infinity/)
  }
})
