/* eslint-disable test/no-import-node-test -- Uses the built-in Node test runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import ts from 'typescript'

const source = readFileSync(new URL('../src/views/proxies/utils/location.ts', import.meta.url), 'utf8')
const compiled = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText
const { locationDetectionMessage, proxyLocationMessage, proxyTestFeedback } = await import(`data:text/javascript,${encodeURIComponent(compiled)}`)
const location = { country: 'JP', region: 'Tokyo', city: 'Tokyo', timezone: 'Asia/Tokyo' }

test('location display distinguishes manual, automatic and failed results without stale timezone', () => {
  assert.equal(proxyLocationMessage({}), '继承全局')
  assert.equal(proxyLocationMessage({ requestLocation: location }), 'Tokyo · Asia/Tokyo')
  assert.equal(proxyLocationMessage({ autoLocation: true }), '尚未识别出口地区')
  assert.equal(proxyLocationMessage({ autoLocation: true, detectedLocation: { location }, lastTest: { success: true } }), 'Tokyo · Asia/Tokyo')
  assert.equal(proxyLocationMessage({ autoLocation: true, detectedLocation: { location } }), '尚未识别出口地区')
  assert.equal(proxyLocationMessage({ autoLocation: true, detectedLocation: { location }, lastTest: { success: false } }), '连接测试失败，出口地区不可用')
  assert.equal(proxyLocationMessage({ autoLocation: true, detectedLocation: { location, exitIpv4: '192.0.2.1' }, lastTest: { success: true, exitIpv4: '192.0.2.2' } }), '出口已变化，请重新识别地区')
  assert.equal(proxyLocationMessage({
    autoLocation: true,
    detectedLocation: { location },
    lastTest: { location: { status: 'failed', message: '查询失败' } },
  }), '查询失败')
  assert.equal(proxyLocationMessage({
    autoLocation: true,
    lastTest: { location: { status: 'conflict' } },
  }), 'IPv4 与 IPv6 出口时区不同，自动地区不可用')
  assert.equal(locationDetectionMessage({ status: 'detected', location }), 'Tokyo · Asia/Tokyo')
})

test('connectivity and location failure are separate feedback states', () => {
  assert.deepEqual(proxyTestFeedback({ success: false, message: '认证失败', location: { status: 'failed', message: '查询失败' } }), { tone: 'error', message: '认证失败' })
  assert.deepEqual(proxyTestFeedback({ success: true, latencyMs: 12 }), { tone: 'success', message: '连接成功，耗时 12 ms' })
  assert.equal(proxyTestFeedback({ success: true, location: { status: 'conflict' } }).tone, 'warning')
  assert.equal(proxyTestFeedback({ success: true, location: { status: 'failed', message: '查询失败' } }).tone, 'warning')
  assert.deepEqual(proxyTestFeedback({ success: true, location: { status: 'detected', location } }), { tone: 'success', message: '连接成功；Tokyo · Asia/Tokyo' })
})
