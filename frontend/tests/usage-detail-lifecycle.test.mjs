/* eslint-disable test/no-import-node-test -- follows the repository Node test runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

const require = createRequire(import.meta.url)
function fixture() {
  const calls = []
  const modules = new Map()
  function load(url) {
    if (modules.has(url.href))
      return modules.get(url.href)
    const exports = {}
    modules.set(url.href, exports)
    const { outputText } = ts.transpileModule(readFileSync(url, 'utf8'), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 } })
    runInNewContext(outputText, { exports, AbortController, require(name) {
      if (name === 'vue')
        return vue
      if (name === '@/api') {
        return { getUsageRecordDetail: (query, options) => new Promise((resolve, reject) => calls.push({ query, options, resolve, reject })) }
      }
      if (name === '@/api/request')
        return { ApiError: class ApiError extends Error {} }
      if (name === '@/utils/async')
        return { errorMessage: error => error.message }
      if (name === '../utils/records')
        return { normalizeUsageRecord: value => value }
      if (name.startsWith('@/'))
        return load(new URL(`../src/${name.slice(2)}.ts`, import.meta.url))
      return require(name)
    } })
    return exports
  }
  const scope = vue.effectScope()
  const state = scope.run(() => load(new URL('../src/views/usage/composables/useUsageRecordDetail.ts', import.meta.url)).useUsageRecordDetail())
  return { calls, state, scope }
}
async function flush() {
  await Promise.resolve()
  await vue.nextTick()
}

test('details open immediately, discard stale responses and stay closed after dismissal', async () => {
  const { calls, state, scope } = fixture()
  try {
    state.handleViewDetail({ id: 'a', outcome: 'failed' })
    assert.equal(state.showDetailModal.value, true)
    assert.equal(state.detailLoading.value, true)
    assert.equal(state.detailIsError.value, true)
    state.handleViewDetail({ id: 'b', outcome: 'succeeded' })
    assert.equal(calls[0].options.signal.aborted, true)
    calls[1].resolve({ id: 'b', logicalOutcome: 'succeeded' })
    await flush()
    calls[0].resolve({ id: 'a', logicalOutcome: 'failed' })
    await flush()
    assert.equal(state.selectedUsageRecord.value.id, 'b')
    state.handleViewDetail({ id: 'c', outcome: 'failed' })
    state.showDetailModal.value = false
    assert.equal(calls[2].options.signal.aborted, true)
    calls[2].resolve({ id: 'c', logicalOutcome: 'failed' })
    await flush()
    assert.equal(state.showDetailModal.value, false)
    assert.equal(state.selectedUsageRecord.value, null)
  }
  finally { scope.stop() }
})

test('failed detail reads offer retry and keep failures separate from record content', async () => {
  const { calls, state, scope } = fixture()
  try {
    state.handleViewDetail({ id: 'a', outcome: 'failed' })
    calls[0].reject(new Error('temporary failure'))
    await flush()
    assert.equal(state.detailError.value, 'temporary failure')
    assert.equal(state.detailLoading.value, false)
    const pending = state.reloadDetail()
    assert.equal(state.detailError.value, '')
    calls[1].resolve({ id: 'a', logicalOutcome: 'failed' })
    await pending
    assert.equal(state.selectedUsageRecord.value.id, 'a')
    assert.equal(calls[1].options.silent, true)
  }
  finally { scope.stop() }
})
