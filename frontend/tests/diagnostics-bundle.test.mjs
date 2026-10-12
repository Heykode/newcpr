/* eslint-disable test/no-import-node-test -- Node's built-in runner matches existing tests. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

const { outputText } = ts.transpileModule(readFileSync(new URL('../src/views/usage/utils/diagnosticsBundle.ts', import.meta.url), 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
})
const module = { exports: {} }
runInNewContext(outputText, {
  exports: module.exports,
  require: () => ({ failureClassText: () => 'transport' }),
})
const { requestDiagnosticsBundle } = module.exports

function detail(events) {
  return {
    requestId: 'req_test',
    trace: { schemaVersion: 1, events, droppedEvents: 0 },
    attempts: [],
    relatedRequests: [],
    metadata: { authorization: 'credential-secret' },
  }
}

test('exports precommit and failure facts while excluding arbitrary payloads', () => {
  const bundle = requestDiagnosticsBundle('req_test', detail([
    { stage: 'provider.precommit.released', data: { reason: 'byte_limit', prefetchedBytes: 131073, waitMs: 4, payload: 'body-secret' } },
    { stage: 'attempt.failed', data: { kind: 'transport', sendState: 'Ambiguous', upstreamStatus: 500, message: 'body-secret' } },
  ]))
  assert.equal(bundle.schemaVersion, 4)
  assert.equal(bundle.trace.events[0].precommitRelease.prefetchedBytes, 131073)
  assert.equal(bundle.trace.events[0].precommitRelease.waitMs, 4)
  assert.equal(bundle.trace.events[1].failure.sendState, 'Ambiguous')
  assert.equal(bundle.trace.events[1].failure.upstreamStatus, 500)
  assert.doesNotMatch(JSON.stringify(bundle), /credential-secret|body-secret/)
})

test('keeps zero measurements and rejects untrusted classifications and invalid numbers', () => {
  const events = [
    { stage: 'provider.precommit.released', data: { reason: 'disabled', prefetchedBytes: 0, waitMs: 0 } },
    { stage: 'provider.precommit.released', data: { reason: 'secret', prefetchedBytes: -1, waitMs: Number.MAX_SAFE_INTEGER + 1 } },
    { stage: 'attempt.failed', data: { kind: 'secret', sendState: 'secret', upstreamStatus: 700 } },
  ]
  const bundle = requestDiagnosticsBundle('req_test', detail(events))
  assert.equal(bundle.trace.events[0].precommitRelease.prefetchedBytes, 0)
  assert.equal(bundle.trace.events[0].precommitRelease.waitMs, 0)
  assert.equal(bundle.trace.events[1].precommitRelease.reason, null)
  assert.equal(bundle.trace.events[1].precommitRelease.prefetchedBytes, null)
  assert.equal(bundle.trace.events[1].precommitRelease.waitMs, null)
  assert.ok(Object.values(bundle.trace.events[2].failure).every(value => value === null))
})

test('never attributes stale modal diagnostics to a different request', () => {
  const bundle = requestDiagnosticsBundle('req_new', detail([{ stage: 'attempt.failed', data: { kind: 'transport' } }]))
  assert.equal(bundle.trace, null)
  assert.equal(bundle.request.source, 'request_id_only')
  assert.equal(bundle.request.requestId, 'req_new')
})
