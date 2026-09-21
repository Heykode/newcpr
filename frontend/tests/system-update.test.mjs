/* eslint-disable test/no-import-node-test -- Exercise the real store using Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import { createPinia, disposePinia } from 'pinia'
import ts from 'typescript'
import * as vue from 'vue'

const require = createRequire(import.meta.url)
const filename = new URL('../src/stores/modules/system-update.ts', import.meta.url)
const { outputText } = ts.transpileModule(readFileSync(filename, 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
})

class ApiError extends Error {
  constructor(status) {
    super(`HTTP ${status}`)
    this.status = status
  }
}

function status(state = 'idle', id = null) {
  return {
    currentVersion: state === 'succeeded' ? '3.2.0' : '3.1.0',
    previousVersion: null,
    needRestart: state === 'succeeded',
    operation: { operationId: id, status: state, kind: 'update' },
  }
}

function harness(t, initialStatus = status()) {
  const event = {
    data: vue.shallowRef(null),
    status: vue.shallowRef('CLOSED'),
    error: vue.shallowRef(null),
    eventSource: vue.shallowRef(null),
    open() {
      event.eventSource.value = {}
      event.status.value = 'OPEN'
    },
    close() {
      event.status.value = 'CLOSED'
      event.eventSource.value = null
    },
  }
  const poll = {
    isActive: vue.shallowRef(false),
    resume() { poll.isActive.value = true },
    pause() { poll.isActive.value = false },
  }
  const calls = []
  const network = {
    current: initialStatus,
    post: async () => {
      network.current = status('running', 'new-operation')
      return { operationId: 'new-operation' }
    },
    get: async () => network.current,
  }
  const exports = {}
  const dependencies = {
    vue,
    '@vueuse/core': {
      useEventSource: () => event,
      useTimeoutPoll: (callback) => {
        poll.tick = callback
        return poll
      },
      until: () => { throw new Error('SSE should open synchronously in this harness') },
    },
    '@/api': {
      getSystemVersion: async () => ({ version: '3.1.0', hasUpdate: true }),
      getSystemUpdateDetail: async () => ({
        hasUpdate: true,
        buildType: 'release',
        updateSupported: true,
        latestVersion: '3.2.0',
        notes: 'Synthetic release notes',
      }),
      getSystemUpdateStatus: async () => {
        calls.push('status')
        return network.get()
      },
      performSystemUpdate: async () => {
        calls.push('post')
        return network.post()
      },
      restartSystem: async () => { throw new Error('No restart is allowed in these tests') },
    },
    '@/api/constants': { API_BASE_URL: '' },
    '@/api/request': { ApiError },
    '@/utils/async': { errorMessage: error => error.message },
  }
  runInNewContext(outputText, {
    exports,
    require: name => dependencies[name] ?? require(name),
  })
  const pinia = createPinia()
  const store = exports.useSystemUpdateStore(pinia)
  t.after(() => disposePinia(pinia))
  return { store, network, poll, event, calls }
}

test('accepted updates remain busy until persisted success and reject duplicate clicks', async (t) => {
  const h = harness(t)
  await h.store.loadSystem()
  assert.equal(h.store.canUpdate, true)
  await h.store.updateNow('v3.2.0')
  assert.equal(h.store.updating, true)
  assert.equal(h.store.updateSuccess, false)
  assert.equal(h.store.needRestart, false)
  assert.equal(h.poll.isActive.value, true)
  assert.equal(await h.store.updateNow('3.2.0'), null)
  assert.equal(h.calls.filter(call => call === 'post').length, 1)
  h.network.current = status('succeeded', 'new-operation')
  await h.poll.tick()
  assert.equal(h.store.updating, false)
  assert.equal(h.store.needRestart, true)
  assert.equal(h.store.updateSuccess, true)
  assert.equal(h.store.canUpdate, false)
  assert.equal(h.poll.isActive.value, false)
})

test('lost POST response is reconciled by status without repeating installation', async (t) => {
  const h = harness(t)
  await h.store.loadSystem()
  h.network.post = async () => {
    h.network.current = status('running', 'accepted-but-disconnected')
    throw new ApiError(0)
  }
  assert.equal(await h.store.updateNow('3.2.0'), null)
  assert.equal(h.store.updating, true)
  assert.equal(h.store.updateError, '')
  await h.poll.tick()
  assert.equal(h.calls.filter(call => call === 'post').length, 1)
})

test('uncertain submission with unchanged operation is not reported as started', async (t) => {
  const h = harness(t)
  await h.store.loadSystem()
  h.network.post = async () => {
    throw new ApiError(503)
  }
  await h.store.updateNow('3.2.0')
  assert.equal(h.store.updating, false)
  assert.match(h.store.updateError, /尚未确认更新任务/)
  assert.equal(h.poll.isActive.value, false)
  assert.equal(h.calls.filter(call => call === 'post').length, 1)
})

test('reopening recovers existing task, filters other events and queries the terminal state', async (t) => {
  const h = harness(t, status('running', 'existing-operation'))
  await h.store.loadSystem()
  assert.equal(h.store.updating, true)
  h.event.data.value = { raw: JSON.stringify({ id: 'other', operationId: 'other-operation' }) }
  await vue.nextTick()
  assert.equal(h.store.updateLogs.length, 0)
  h.network.current = status('succeeded', 'existing-operation')
  h.event.data.value = { raw: JSON.stringify({
    id: 'done',
    operationId: 'existing-operation',
    terminal: true,
    message: 'Synthetic completed update',
  }) }
  await new Promise(resolve => setImmediate(resolve))
  assert.equal(h.store.needRestart, true)
  assert.equal(h.store.updateLogs.length, 1)
  assert.equal(h.calls.includes('post'), false)
})

test('temporary status failures retain busy state; revoked admin session stops polling', async (t) => {
  const h = harness(t, status('running', 'existing-operation'))
  await h.store.loadSystem()
  h.network.get = async () => {
    throw new ApiError(503)
  }
  await h.poll.tick()
  assert.equal(h.store.updating, true)
  assert.equal(h.poll.isActive.value, true)
  assert.match(h.store.updateStreamError, /正在重试/)
  h.network.get = async () => {
    throw new ApiError(401)
  }
  await h.poll.tick()
  assert.equal(h.store.phase.kind, 'failed')
  assert.equal(h.poll.isActive.value, false)
  assert.equal(h.event.status.value, 'CLOSED')
  assert.equal(h.calls.includes('post'), false)
})

test('failed persisted task releases busy controls without claiming a successful update', async (t) => {
  const h = harness(t, {
    ...status('failed', 'failed-operation'),
    operation: { ...status('failed', 'failed-operation').operation, error: 'Checksum mismatch' },
  })
  await h.store.loadSystem()
  assert.equal(h.store.updating, false)
  assert.equal(h.store.updateSuccess, false)
  assert.equal(h.store.needRestart, false)
  assert.equal(h.store.updateError, 'Checksum mismatch')
  assert.equal(h.poll.isActive.value, false)
})
