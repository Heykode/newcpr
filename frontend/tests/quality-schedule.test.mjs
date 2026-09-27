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
const { readSchedule, writeSchedule, scheduleSummary } = exports

test('quality scheduling preserves the existing six-hour default and hourly presets', () => {
  for (const interval of ['1', '3', '6', '12']) {
    const cron = writeSchedule(interval, '09:00')
    assert.equal(readSchedule(cron).frequency, interval)
  }
  assert.equal(writeSchedule('6', '09:00'), '0 */6 * * *')
  assert.equal(readSchedule('0 */1 * * *').frequency, '1')
  assert.equal(scheduleSummary('6', ''), '每天 00:00、06:00、12:00、18:00')
})

test('daily schedules round-trip midnight, minute precision and end of day', () => {
  for (const time of ['00:00', '00:05', '08:30', '23:59']) {
    const cron = writeSchedule('daily', time)
    assert.equal(readSchedule(cron).frequency, 'daily')
    assert.equal(readSchedule(cron).time, time)
  }
  assert.equal(writeSchedule('daily', '08:30'), '30 8 * * *')
  assert.equal(scheduleSummary('daily', '08:30'), '每天 08:30')
})

test('custom expressions are not coerced into a simpler and different schedule', () => {
  for (const cron of ['15 9 * * 1-5', '0 */2 * * *', '0 0 1 * *', '*/30 * * * *', '0 0 9 * * *', '0 24 * * *', '60 9 * * *', '']) {
    assert.equal(readSchedule(cron).frequency, 'custom', cron)
  }
  assert.equal(writeSchedule('custom', '08:30'), null)
})

test('invalid or incomplete daily times cannot replace the saved cron expression', () => {
  for (const time of ['', '08:', '24:00', '12:60', '9:30', '-1:00', '08:30:01'])
    assert.equal(writeSchedule('daily', time), null)
})
