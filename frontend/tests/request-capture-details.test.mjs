/* eslint-disable test/no-import-node-test -- follows the project's Node regression runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

const tick = () => new Promise(resolve => setImmediate(resolve))
const file = new URL('../src/views/usage/composables/useRequestCaptureDetails.ts', import.meta.url)
const code = ts.transpileModule(readFileSync(file, 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
}).outputText

function harness(t) {
  const lookups = []
  const reads = []
  const exports = {}
  const api = {
    getCapturesForRequest: (requestId, options) => new Promise((resolve, reject) => lookups.push({ requestId, ...options, resolve, reject })),
    readRequestCapture: (id, offset, options) => new Promise((resolve, reject) => reads.push({ id, offset, ...options, resolve, reject })),
  }
  runInNewContext(code, { exports, AbortController, require: name => name === 'vue' ? vue : api })
  const requestId = vue.ref('request-a')
  const scope = vue.effectScope()
  const state = scope.run(() => exports.useRequestCaptureDetails(() => requestId.value))
  t.after(() => scope.stop())
  return { state, requestId, lookups, reads, scope }
}
function record(requestId, id = `capture-${requestId}`) {
  return {
    id,
    requestId,
    taskId: 'fixture-task',
    bytes: 30,
    incomplete: false,
    createdAt: '2026-01-01T00:00:00Z',
  }
}

test('detail loads request-local metadata, then bounded pages, never global capture status', async (t) => {
  const h = harness(t)
  assert.equal(h.lookups.length, 1)
  assert.equal(h.lookups[0].requestId, 'request-a')
  assert.equal(h.reads.length, 0)
  h.lookups[0].resolve([record('request-a')])
  await tick()
  assert.equal(h.reads[0].id, 'capture-request-a')
  assert.equal(h.reads[0].offset, 0)
  h.reads[0].resolve({ text: 'first', nextOffset: 262144 })
  await tick()
  h.state.offset.value = h.state.page.value.nextOffset
  await tick()
  assert.equal(h.state.page.value, null)
  assert.equal(h.reads[1].offset, 262144)
  h.reads[1].resolve({ text: 'second', nextOffset: null })
  await tick()
  assert.equal(h.state.page.value.text, 'second')
})

test('switching requests discards late metadata and late bodies, even when transport ignores abort', async (t) => {
  const h = harness(t)
  h.requestId.value = 'request-b'
  await tick()
  assert.ok(h.lookups[0].signal.aborted)
  h.lookups[0].resolve([record('request-a')])
  await tick()
  assert.equal(h.reads.length, 0)
  h.lookups[1].resolve([record('request-b')])
  await tick()
  h.requestId.value = 'request-c'
  await tick()
  assert.ok(h.reads[0].signal.aborted)
  h.reads[0].resolve({ text: 'must not leak request-b', nextOffset: null })
  h.lookups[2].resolve([])
  await tick()
  assert.equal(h.state.page.value, null)
  assert.equal(h.state.records.value.length, 0)
})

test('closing detail aborts reads and clears sensitive content without late resurrection', async (t) => {
  const h = harness(t)
  h.lookups[0].resolve([record('request-a')])
  await tick()
  h.reads[0].resolve({ text: 'sensitive fixture', nextOffset: 30 })
  await tick()
  assert.equal(h.state.page.value.text, 'sensitive fixture')
  h.state.offset.value = 30
  await tick()
  h.scope.stop()
  assert.ok(h.reads[1].signal.aborted)
  assert.equal(h.state.page.value, null)
  assert.equal(h.state.records.value.length, 0)
  h.reads[1].resolve({ text: 'late fixture', nextOffset: null })
  await tick()
  assert.equal(h.state.page.value, null)
})

test('wrong-request metadata never triggers a body read; failed reads have an explicit retry', async (t) => {
  const h = harness(t)
  h.lookups[0].resolve([record('wrong-request')])
  await tick()
  assert.ok(h.state.error.value)
  assert.equal(h.reads.length, 0)
  h.state.refresh()
  await tick()
  h.lookups[1].resolve([record('request-a')])
  await tick()
  h.reads[0].reject(new Error('fixture read failure'))
  await tick()
  assert.ok(h.state.readError.value)
  assert.equal(h.state.page.value, null)
  h.state.retry()
  await tick()
  h.reads[1].resolve({ text: 'retried', nextOffset: null })
  await tick()
  assert.equal(h.state.readError.value, '')
  assert.equal(h.state.page.value.text, 'retried')
})

test('capture selection resets pagination and drops the previous record body', async (t) => {
  const h = harness(t)
  h.lookups[0].resolve([record('request-a', 'capture-1'), record('request-a', 'capture-2')])
  await tick()
  h.reads[0].resolve({ text: 'old body', nextOffset: 100 })
  await tick()
  h.state.offset.value = 100
  await tick()
  h.state.selectedId.value = 'capture-2'
  await tick()
  assert.equal(h.reads.at(-1).id, 'capture-2')
  assert.equal(h.reads.at(-1).offset, 0)
  assert.equal(h.state.page.value, null)
})
