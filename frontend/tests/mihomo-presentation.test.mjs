/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

const exports = {}
const { outputText } = ts.transpileModule(readFileSync(new URL('../src/views/proxies/mihomoPresentation.ts', import.meta.url), 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
})
runInNewContext(outputText, { exports })
const { countryBlockReason, qualityStatus, regionLabel, regionMatches, sameCountryFilter } = exports
const filter = { mode: 'exclude', codes: ['CN', 'HK'], allowUnknown: false, dynamicProviderManaged: false }
const node = { name: 'node-example', displayName: '新加坡 01', countryBlocked: true, dynamic: false, countryCode: null, countryCheckedAt: null, countryError: null }

test('Singapore labels and diagnostic snapshots do not override the authoritative region decision', () => {
  assert.equal(countryBlockReason({ ...node, check: { countryCode: 'SG', countryName: 'Singapore' } }, filter), '地区未检测，暂不准入')
  assert.equal(countryBlockReason({ ...node, countryCode: 'SG', countryBlocked: false }, filter), null)
  assert.match(countryBlockReason({ ...node, countryCode: 'CN' }, filter), /已排除地区：.*CN/)
  assert.match(countryBlockReason({ ...node, countryCode: 'SG' }, { ...filter, mode: 'include' }), /不在允许地区内：.*SG/)
})

test('unknown, failed and dynamic regions have distinct reasons without inventing exclusions', () => {
  assert.equal(countryBlockReason({ ...node, countryError: 'lookup_failed' }, filter), '地区检测失败，暂不准入')
  assert.equal(countryBlockReason({ ...node, countryCheckedAt: '2026-09-28T00:00:00Z' }, filter), '地区未知，暂不准入')
  assert.equal(countryBlockReason({ ...node, dynamic: true, countryCode: 'SG' }, filter), '动态地区未准入')
  assert.equal(countryBlockReason({ ...node, countryCode: 'SG' }, filter), '地区规则限制，暂不准入')
  assert.equal(countryBlockReason({ ...node, countryBlocked: false }, filter), null)
})

test('region search accepts Chinese, English and codes without mutating configured codes', () => {
  assert.match(regionLabel('SG'), /新加坡 SG/)
  for (const term of [' 新加坡 ', 'singapore', 'SG', 'sg'])
    assert.equal(regionMatches('SG', term), true)
  assert.equal(regionMatches('SG', '香港'), false)
  assert.equal(regionLabel('invalid'), 'invalid')
  assert.equal(regionMatches('invalid', 'invalid'), true)
})

test('draft confirmation compares all policy fields and is independent of code order', () => {
  assert.equal(sameCountryFilter(filter, { ...filter, codes: ['HK', 'CN'] }), true)
  for (const patch of [{ mode: 'include' }, { codes: ['SG'] }, { allowUnknown: true }, { dynamicProviderManaged: true }])
    assert.equal(sameCountryFilter(filter, { ...filter, ...patch }), false)
  assert.deepEqual(filter.codes, ['CN', 'HK'])
})

test('a failed base probe cannot look healthy merely because its score is B', () => {
  assert.equal(qualityStatus({ grade: 'B', score: 78, checks: [{ status: 'fail', name: 'base_connectivity' }] }), '存在失败')
  assert.equal(qualityStatus({ checks: [{ status: 'warn' }] }), '存在告警')
  assert.equal(qualityStatus({ checks: [{ status: 'fail' }, { status: 'challenge' }] }), '遇到挑战')
  assert.equal(qualityStatus({ checks: [{ status: 'pass', httpStatus: 401 }] }), '连通检测通过')
  for (const value of [null, undefined, { checks: [] }])
    assert.equal(qualityStatus(value), '未完成检测')
})
