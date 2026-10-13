/* eslint-disable test/no-import-node-test -- offline component lifecycle regression. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { setImmediate } from 'node:timers/promises'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import { parse } from 'vue/compiler-sfc'

function harness() {
  const source = readFileSync(new URL('../src/views/accounts/components/AccountEnrollmentProgress.vue', import.meta.url), 'utf8')
  const { outputText } = ts.transpileModule(parse(source).descriptor.scriptSetup.content, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  const timers = new Map()
  const requests = []
  const events = []
  let dispose
  runInNewContext(outputText, {
    exports: {},
    AbortController,
    defineProps: () => ({ ids: ['entry'] }),
    defineEmits: () => event => events.push(event),
    setTimeout: (callback) => {
      timers.set(1, callback)
      return 1
    },
    clearTimeout: id => timers.delete(id),
    require: name => ({
      'vue': {
        shallowRef: value => ({ value }),
        watch: (_source, callback) => callback(),
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
    dispose: () => dispose(),
    async respond(status, recovery, enrollmentPending = true) {
      requests.at(-1).resolve({
        settings: { paused: false },
        items: [{ id: 'entry', status, enrollmentPending, recovery: { state: recovery }, poolStatus: enrollmentPending ? 'absent' : 'synced' }],
      })
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
