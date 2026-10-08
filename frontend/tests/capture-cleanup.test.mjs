/* eslint-disable test/no-import-node-test -- Node regression runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

const tick = () => new Promise(resolve => setImmediate(resolve))
const running = { id: 'fixture-job', captureOnly: true, status: 'running', removed: 2 }
function harness(overrides = {}) {
  let mounted
  let unmount
  let timer
  let posts = 0
  let cancelled = 0
  let finished = 0
  const api = {
    getCleanupState: async () => ({ job: null }),
    startCaptureCleanup: async () => {
      posts++
      return running
    },
    cancelCleanup: async () => { cancelled++ },
    ...overrides,
  }
  const exports = {}
  const source = readFileSync(new URL('../src/views/usage/composables/useCaptureCleanup.ts', import.meta.url), 'utf8')
  runInNewContext(ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText, {
    exports,
    AbortController,
    setTimeout: fn => (timer = fn),
    clearTimeout: () => { timer = undefined },
    require(name) {
      if (name === 'vue')
        return { ...vue, onMounted: fn => (mounted = fn), onBeforeUnmount: fn => (unmount = fn) }
      if (name === '@/api/modules/log-cleanup')
        return api
      if (name === '@/utils/async')
        return { errorMessage: error => error.message }
      throw new Error(name)
    },
  })
  const state = exports.useCaptureCleanup(() => {
    finished++
  })
  return { state, api, mount: () => mounted(), unmount: () => unmount(), timer: () => timer, posts: () => posts, cancelled: () => cancelled, finished: () => finished }
}

test('capture cleanup unmount cancels reads only and reopening resumes persisted progress', async () => {
  const h = harness()
  h.mount()
  await tick()
  await h.state.start()
  assert.equal(h.posts(), 1)
  assert.equal(h.state.running.value, true)
  h.unmount()
  assert.equal(h.timer(), undefined)
  assert.equal(h.cancelled(), 0)
  const reopened = harness({ getCleanupState: async () => ({ job: running }) })
  reopened.mount()
  await tick()
  assert.equal(reopened.state.running.value, true)
  assert.equal(reopened.posts(), 0)
  reopened.api.getCleanupState = async () => ({ job: { ...running, status: 'succeeded' } })
  await reopened.state.refresh()
  assert.equal(reopened.finished(), 1)
  reopened.unmount()
})

test('an older status read cannot overwrite an accepted job or trigger duplicate starts', async () => {
  let resolveRead
  const h = harness({ getCleanupState: () => new Promise(resolve => (resolveRead = resolve)) })
  h.mount()
  await h.state.start()
  resolveRead({ job: null })
  await tick()
  assert.equal(h.state.job.value.id, running.id)
  await h.state.start()
  assert.equal(h.posts(), 1)
  h.unmount()
})

test('ambiguous starts are never automatically replayed and other cleanup jobs block start', async () => {
  let posts = 0
  const h = harness({ startCaptureCleanup: async () => {
    posts++
    throw new Error('uncertain')
  } })
  await h.state.start()
  assert.match(h.state.error.value, /uncertain/)
  await h.state.refresh()
  assert.equal(posts, 1)
  h.api.getCleanupState = async () => ({ job: { ...running, captureOnly: false } })
  await h.state.refresh()
  assert.equal(h.state.occupied.value, true)
  assert.equal(h.state.running.value, false)
  await h.state.start()
  assert.equal(posts, 1)
  h.unmount()
})
