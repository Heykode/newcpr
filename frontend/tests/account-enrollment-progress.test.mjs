/* eslint-disable test/no-import-node-test -- offline component lifecycle regression. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { setImmediate } from 'node:timers/promises'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import { parse } from 'vue/compiler-sfc'

function harness(enrollments = [{ id: 'entry', credentialTarget: 'codex' }]) {
  const source = readFileSync(new URL('../src/views/accounts/components/AccountEnrollmentProgress.vue', import.meta.url), 'utf8')
  const { outputText } = ts.transpileModule(parse(source).descriptor.scriptSetup.content, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  const timers = new Map()
  const requests = []
  const events = []
  const refs = []
  const props = { enrollments }
  let dispose
  let update
  runInNewContext(outputText, {
    exports: {},
    AbortController,
    defineProps: () => props,
    defineEmits: () => event => events.push(event),
    setTimeout: (callback) => {
      timers.set(1, callback)
      return 1
    },
    clearTimeout: id => timers.delete(id),
    require: name => ({
      'vue': {
        shallowRef: (value) => {
          const ref = { value }
          refs.push(ref)
          return ref
        },
        watch: (_source, callback) => {
          update = callback
          callback(props.enrollments)
        },
        onScopeDispose: (callback) => { dispose = callback },
      },
      '@/api/modules/relogin': {
        getRelogin: options => new Promise(resolve => requests.push({ options, resolve })),
      },
      '@/utils/async': { errorMessage: () => 'read failed' },
    }[name] ?? {}),
  })
  return {
    timers,
    requests,
    events,
    rows: () => structuredClone(refs[0].value),
    changeEnrollments(enrollments) {
      const previous = props.enrollments
      props.enrollments = enrollments
      update(enrollments, previous)
    },
    dispose: () => dispose(),
    async respond(status, recovery, enrollmentPending = true) {
      await this.respondRows([{ id: 'entry', status, enrollmentPending, recovery: { state: recovery }, poolStatus: enrollmentPending ? 'absent' : 'synced' }])
    },
    async respondRows(items) {
      requests.at(-1).resolve({ settings: { paused: false }, items })
      await setImmediate()
    },
    tick() {
      const callback = timers.get(1)
      timers.delete(1)
      callback()
    },
  }
}

test('enrollment keeps polling through retry cooldown and refreshes accounts on success', async () => {
  const h = harness()
  for (const [status, recovery] of [['queued', 'waiting'], ['failed', 'cooldown'], ['failed', 'waiting'], ['running', 'running']]) {
    await h.respond(status, recovery)
    assert.equal(h.timers.size, 1)
    h.tick()
  }
  await h.respond('ready', 'idle', false)
  assert.deepEqual(h.events, ['completed'])
  assert.equal(h.timers.size, 0)
  h.dispose()
})

test('Excel enrollment polls and displays only the Excel channel even when Codex is synced', async () => {
  const h = harness([{ id: 'entry', credentialTarget: 'excel' }])
  const row = {
    id: 'entry',
    email: 'import@example.invalid',
    status: 'ready',
    poolStatus: 'synced',
    message: 'Codex ready',
    excel: { id: 'entry', email: 'import@example.invalid', status: 'failed', poolStatus: 'absent', enrollmentPending: true, message: 'Excel failed', recovery: { state: 'cooldown', message: 'Excel retry' } },
  }
  await h.respondRows([row])
  assert.equal(h.rows()[0].message, 'Excel failed')
  assert.equal(h.rows()[0].recovery.message, 'Excel retry')
  assert.deepEqual(h.events, [])
  assert.equal(h.timers.size, 1)
  h.tick()
  await h.respondRows([{ ...row, excel: { ...row.excel, status: 'ready', poolStatus: 'synced', enrollmentPending: false } }])
  assert.deepEqual(h.events, ['completed'])
  assert.equal(h.timers.size, 0)
  h.dispose()
})

test('same account Codex and Excel enrollments complete independently', async () => {
  const h = harness([{ id: 'entry', credentialTarget: 'codex' }, { id: 'entry', credentialTarget: 'excel' }])
  const row = {
    id: 'entry',
    status: 'ready',
    poolStatus: 'synced',
    excel: { id: 'entry', status: 'queued', poolStatus: 'absent', enrollmentPending: true },
  }
  await h.respondRows([row])
  assert.equal(h.rows().length, 2)
  assert.deepEqual(h.events, ['completed'])
  assert.equal(h.timers.size, 1)
  h.tick()
  await h.respondRows([{ ...row, excel: { ...row.excel, status: 'ready', poolStatus: 'synced', enrollmentPending: false } }])
  assert.deepEqual(h.events, ['completed', 'completed'])
  assert.equal(h.timers.size, 0)
  h.dispose()
})

test('Excel terminal failure does not inherit Codex pending or synced state', async () => {
  for (const status of ['pending', 'ready']) {
    const h = harness([{ id: 'entry', credentialTarget: 'excel' }])
    await h.respondRows([{
      id: 'entry',
      status,
      poolStatus: 'synced',
      excel: { id: 'entry', status: 'failed', poolStatus: 'absent', message: 'Excel needs interaction', enrollmentPending: true, recovery: { state: 'manual_required' } },
    }])
    assert.equal(h.rows()[0].message, 'Excel needs interaction')
    assert.deepEqual(h.events, [])
    assert.equal(h.timers.size, 0)
    h.dispose()
  }
})

test('missing Excel channel waits for its own record without completing Codex', async () => {
  const h = harness([{ id: 'entry', credentialTarget: 'excel' }])
  await h.respondRows([{ id: 'entry', status: 'ready', poolStatus: 'synced' }])
  assert.equal(h.rows().length, 0)
  assert.deepEqual(h.events, [])
  assert.equal(h.timers.size, 1)
  h.dispose()
})

test('a new enrollment of the same channel can complete again', async () => {
  const h = harness([{ id: 'entry', credentialTarget: 'excel' }])
  const row = { id: 'entry', excel: { id: 'entry', status: 'ready', poolStatus: 'synced' } }
  await h.respondRows([row])
  h.changeEnrollments([{ id: 'entry', credentialTarget: 'excel' }])
  await h.respondRows([row])
  assert.deepEqual(h.events, ['completed', 'completed'])
  h.dispose()
})

test('adding another channel does not repeat an already observed completion', async () => {
  const codex = { id: 'entry', credentialTarget: 'codex' }
  const h = harness([codex])
  await h.respond('ready', 'idle', false)
  h.changeEnrollments([codex, { id: 'entry', credentialTarget: 'excel' }])
  await h.respondRows([{
    id: 'entry',
    status: 'ready',
    poolStatus: 'synced',
    excel: { id: 'entry', status: 'queued', poolStatus: 'absent', enrollmentPending: true },
  }])
  assert.deepEqual(h.events, ['completed'])
  assert.equal(h.timers.size, 1)
  h.dispose()
})

test('switching enrollment channels rejects the old progress response', async () => {
  const h = harness()
  const old = h.requests[0]
  h.changeEnrollments([{ id: 'entry', credentialTarget: 'excel' }])
  assert.equal(old.options.signal.aborted, true)
  old.resolve({ settings: { paused: false }, items: [{ id: 'entry', status: 'ready', poolStatus: 'synced' }] })
  await setImmediate()
  assert.deepEqual(h.rows(), [])
  assert.deepEqual(h.events, [])
  await h.respondRows([{ id: 'entry', excel: { id: 'entry', status: 'failed', poolStatus: 'absent', message: 'Excel failed' } }])
  assert.equal(h.rows()[0].message, 'Excel failed')
  assert.deepEqual(h.events, [])
  h.dispose()
})

test('terminal or uncertain enrollment does not poll as if another attempt were pending', async () => {
  for (const [status, recovery] of [['failed', 'retry_limit'], ['failed', 'manual_required'], ['uncertain', 'uncertain'], ['awaiting_workspace', 'awaiting_workspace']]) {
    const h = harness()
    await h.respond(status, recovery)
    assert.equal(h.timers.size, 0)
    assert.deepEqual(h.events, [])
    h.dispose()
  }
})

test('leaving the page cancels only progress reads and ignores late retry responses', async () => {
  const h = harness()
  h.dispose()
  assert.equal(h.requests[0].options.signal.aborted, true)
  await h.respond('failed', 'cooldown')
  assert.equal(h.timers.size, 0)
  assert.deepEqual(h.events, [])
})
