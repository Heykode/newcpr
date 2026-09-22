/* eslint-disable test/no-import-node-test -- this regression uses Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

function harness() {
  const calls = []
  const notices = []
  let queued = 0
  let allowed = true
  const accountId = vue.ref('acct_a')
  const exports = {}
  const filename = new URL('../src/views/accounts/composables/useTurnStateProbe.ts', import.meta.url)
  const { outputText } = ts.transpileModule(readFileSync(filename, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  const dependencies = {
    vue,
    '@/api': {
      requestAccountTurnStateProbe: (body, options) => new Promise((resolve, reject) => {
        calls.push({ body, options, resolve, reject })
      }),
    },
    '@/components/base/BaseToast': {
      toast: { success: value => notices.push(value), error: value => notices.push(value) },
    },
    '@/utils/async': { errorMessage: error => error.message },
  }
  runInNewContext(outputText, { exports, require: name => dependencies[name] })
  const scope = vue.effectScope()
  const api = scope.run(() => exports.useTurnStateProbe({
    accountId: () => accountId.value,
    canProbe: () => allowed,
    onQueued: () => {
      queued += 1
    },
  }))
  return {
    api,
    calls,
    notices,
    accountId,
    scope,
    deny: () => {
      allowed = false
    },
    queued: () => queued,
  }
}

test('manual probe coalesces synchronous clicks but allows different models', async () => {
  const h = harness()
  const first = h.api.requestProbe('model-a')
  await h.api.requestProbe('model-a')
  const other = h.api.requestProbe('model-b')
  assert.equal(h.calls.length, 2)
  assert.equal(JSON.stringify(h.calls[0].body), '{"accountId":"acct_a","modelId":"model-a"}')
  assert.equal(h.calls[0].options.silent, true)
  h.calls[0].resolve({ status: 'queued' })
  h.calls[1].resolve({ status: 'already_running' })
  await Promise.all([first, other])
  assert.equal(h.queued(), 2)
  assert.equal(h.api.pending.size, 0)
  assert.deepEqual(h.notices, ['State 探测已排队', '该模型正在排队或采集中'])
  h.deny()
  await h.api.requestProbe('model-c')
  assert.equal(h.calls.length, 2)
  h.scope.stop()
})

test('old account and disposed replies cannot refresh or unlock a newer request', async () => {
  const h = harness()
  const old = h.api.requestProbe('model-a')
  h.accountId.value = 'acct_b'
  const current = h.api.requestProbe('model-a')
  h.calls[0].resolve({ status: 'queued' })
  await old
  assert.equal(h.api.pending.has('model-a'), true)
  assert.equal(h.queued(), 0)
  h.scope.stop()
  h.calls[1].resolve({ status: 'queued' })
  await current
  assert.equal(h.queued(), 0)
  assert.equal(h.notices.length, 0)
})

test('failed submissions unlock without automatic retries or success refresh', async () => {
  const h = harness()
  const run = h.api.requestProbe('model-a')
  h.calls[0].reject(new Error('冷却中'))
  await run
  assert.equal(h.calls.length, 1)
  assert.equal(h.api.pending.size, 0)
  assert.equal(h.queued(), 0)
  assert.deepEqual(h.notices, ['冷却中'])
  h.scope.stop()
})
