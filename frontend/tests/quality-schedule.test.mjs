/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

const { outputText } = ts.transpileModule(readFileSync(new URL('../src/views/quality-ops/schedule.ts', import.meta.url), 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
})
const exports = {}
runInNewContext(outputText, { exports })
const { qualityScheduleSummary, DEFAULT_QUALITY_INTERVAL_SECONDS, MIN_QUALITY_INTERVAL_SECONDS, MAX_QUALITY_INTERVAL_SECONDS } = exports

test('new quality intervals default to sixty seconds with bounded integer input', () => {
  assert.equal(DEFAULT_QUALITY_INTERVAL_SECONDS, 60)
  assert.equal(MIN_QUALITY_INTERVAL_SECONDS, 5)
  assert.equal(MAX_QUALITY_INTERVAL_SECONDS, 31_536_000)
})

test('seconds take precedence without cron approximation or timezone dependence', () => {
  for (const intervalSeconds of [5, 17, 60, 90, 31_536_000])
    assert.equal(qualityScheduleSummary({ intervalSeconds, cron: '0 */6 * * *', timezone: 'UTC' }), `每 ${intervalSeconds} 秒`)
})

test('legacy schedules retain original expression and timezone without conversion', () => {
  for (const cron of ['15 9 * * 1-5', '0 */6 * * *', '30 8 * * *', '0 0 1 * *']) {
    const config = { cron, timezone: 'Asia/Shanghai' }
    assert.equal(qualityScheduleSummary(config), `原定时：${cron}（Asia/Shanghai）`)
    assert.equal(qualityScheduleSummary({ ...config, intervalSeconds: null }), qualityScheduleSummary(config))
    assert.equal(config.intervalSeconds, undefined)
  }
})
