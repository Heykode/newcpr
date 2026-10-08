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
const { usagePerformanceDetails, usageLatencyDetails, usageTokenDetails } = module.exports
function record(overrides = {}) {
  return {
    latencyMs: 3500,
    firstTokenLatencyMs: 1000,
    latencyDetails: { firstTokenMs: 500, firstEventMs: 100 },
    tokenDetails: { outputTokens: 125 },
    ...overrides,
  }
}

test('throughput uses complete request duration including reasoning and prior attempts', () => {
  const result = usagePerformanceDetails(record())
  assert.equal(result.throughputDisplay, '35.7 tok/s')
  assert.equal(result.firstTokenDisplay, '1000ms')
  assert.equal(usageLatencyDetails(record()).firstOutputDisplay, '1000ms')
  assert.equal(usagePerformanceDetails(record({ firstTokenLatencyMs: null })).throughputDisplay, '35.7 tok/s')
  assert.equal(usagePerformanceDetails(record({ firstTokenLatencyMs: 0 })).throughputDisplay, '35.7 tok/s')
})

test('fresh input display subtracts both caches without changing raw usage, totals or billing', () => {
  const tokenDetails = Object.freeze({
    inputTokens: 10000,
    cachedTokens: 7000,
    cacheWriteTokens: 1000,
    outputTokens: 500,
    totalTokens: 10500,
    inputTokensDisplay: '10,000',
    cachedTokensDisplay: '7,000',
    cacheWriteTokensDisplay: '1,000',
  })
  const value = record({ tokenDetails, billing: { totalAmount: '1.25' } })
  const projected = usageTokenDetails(value)
  assert.equal(projected.inputTokens, 2000)
  assert.equal(projected.inputTokensDisplay, '2,000')
  assert.equal(projected.totalTokens, 10500)
  assert.equal(projected.cachedTokensDisplay, '7,000')
  assert.equal(projected.cacheWriteTokensDisplay, '1,000')
  assert.equal(value.tokenDetails.inputTokens, 10000)
  assert.equal(value.billing.totalAmount, '1.25')
  assert.equal(usageTokenDetails(value).inputTokens, 2000)
})

test('fresh input display preserves unknown input and clamps cache underflow to zero', () => {
  for (const [inputTokens, cachedTokens, cacheWriteTokens, expected] of [
    [null, 5, 2, null],
    [10, null, null, 10],
    [10, 8, 5, 0],
    [0, 0, 0, 0],
  ]) {
    const projected = usageTokenDetails(record({ tokenDetails: {
      inputTokens,
      cachedTokens,
      cacheWriteTokens,
      inputTokensDisplay: 'missing',
    } }))
    assert.equal(projected.inputTokens, expected)
    assert.equal(projected.inputTokensDisplay, expected === null ? 'missing' : String(expected))
  }
})

test('missing or invalid measurements never invent throughput or substitute the first lifecycle event', () => {
  for (const overrides of [
    { tokenDetails: null },
    { tokenDetails: { outputTokens: 0 } },
    { tokenDetails: { outputTokens: -1 } },
    { tokenDetails: { outputTokens: Number.NaN } },
    { tokenDetails: { outputTokens: Number.POSITIVE_INFINITY } },
    { latencyMs: 0 },
    { latencyMs: -1 },
    { latencyMs: null },
  ]) {
    const value = usagePerformanceDetails(record(overrides)).throughputDisplay
    assert.equal(value, '\u2014')
    assert.doesNotMatch(value, /NaN|Infinity/)
  }
})
