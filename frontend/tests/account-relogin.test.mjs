/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'
import { compileScript, parse } from 'vue/compiler-sfc'
import { renderToString } from 'vue/server-renderer'

const require = createRequire(import.meta.url)
function load(filename, dependencies = {}, globals = {}) {
  const source = readFileSync(filename, 'utf8')
  const content = filename.pathname.endsWith('.vue')
    ? compileScript(parse(source, { filename: filename.pathname }).descriptor, { id: 'account-relogin', inlineTemplate: true }).content
    : source
  const { outputText } = ts.transpileModule(content, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024, esModuleInterop: true },
  })
  const exports = {}
  runInNewContext(outputText, { exports, AbortController, ...globals, require: name => dependencies[name] ?? require(name) })
  return exports
}

function deferred() {
  let resolve
  let reject
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}
async function flush() {
  for (let i = 0; i < 12; i++)
    await Promise.resolve()
  await vue.nextTick()
}
const account = { id: 'acct-a', name: 'sample', email: 'sample@example.invalid' }
function action(overrides = {}) {
  return {
    accountId: account.id,
    entryId: 'entry-a',
    revision: 1,
    target: { account_id: account.id, credential_revision: 1, user_id: 'user-a', workspace_id: 'workspace-a' },
    status: 'pending',
    message: '',
    busy: false,
    blockedReason: null,
    syncedAt: null,
    ...overrides,
  }
}
function harness(t) {
  const reads = []
  const writes = []
  const timers = new Map()
  const notices = []
  let reloads = 0
  let nextTimer = 0
  const asyncAction = load(new URL('../src/composables/useAsyncAction.ts', import.meta.url), {
    '@/api/request': { ApiError: class extends Error {} },
    '@/components/base/BaseToast': { toast: { error() {} } },
    '@/utils/async': { errorMessage: error => error.message },
  })
  const module = load(new URL('../src/views/accounts/composables/useAccountRelogin.ts', import.meta.url), {
    '@/api/modules/relogin': {
      getAccountReloginActions(ids, options) {
        const read = { ...deferred(), ids, options }
        reads.push(read)
        return read.promise
      },
      queueAccountRelogin(data) {
        const write = { ...deferred(), data }
        writes.push(write)
        return write.promise
      },
    },
    '@/components/base/BaseToast': { toast: { error: text => notices.push(text), success: text => notices.push(text) } },
    '@/composables/useAsyncAction': asyncAction,
    '@/utils/async': { errorMessage: error => error.message },
  }, {
    setTimeout(callback, delay) {
      const id = ++nextTimer
      timers.set(id, { callback, delay })
      return id
    },
    clearTimeout: id => timers.delete(id),
  })
  const accounts = vue.shallowRef([account])
  const scope = vue.effectScope()
  t.after(() => scope.stop())
  const state = scope.run(() => module.useAccountRelogin({
    accounts,
    reload: async () => {
      reloads++
    },
  }))
  return {
    state,
    accounts,
    scope,
    reads,
    writes,
    timers,
    notices,
    reloads: () => reloads,
    async reply(items = [action()]) {
      reads.at(-1).resolve(items)
      await flush()
    },
    tick() {
      assert.equal(timers.size, 1)
      const [id, timer] = [...timers][0]
      timers.delete(id)
      timer.callback()
      return timer.delay
    },
  }
}

test('account action API sends exact target and supports only read cancellation', async () => {
  const requests = []
  const api = load(new URL('../src/api/modules/relogin.ts', import.meta.url), {
    '../request': config => requests.push(config),
  })
  const signal = new AbortController().signal
  await api.getAccountReloginActions(['acct-a'], { signal, silent: true })
  const body = { entryId: 'entry-a', revision: 1, target: action().target }
  await api.queueAccountRelogin(body)
  assert.equal(requests[0].url, '/api/admin/relogin/accounts/query')
  assert.equal(requests[0].method, 'POST')
  assert.equal(requests[0].signal, signal)
  assert.equal(requests[0].silent, true)
  assert.equal(requests[1].url, '/api/admin/relogin/accounts/queue')
  assert.equal(requests[1].data, body)
  assert.equal(requests[1].signal, undefined)
})

test('confirmation cancellation does nothing; confirmed job queues once and refreshes after success', async (t) => {
  const h = harness(t)
  await h.reply()
  h.state.request(account)
  h.state.open.value = false
  await h.state.confirm()
  assert.equal(h.writes.length, 0)
  h.state.request(account)
  const confirmation = h.state.confirm()
  await h.state.confirm()
  assert.equal(h.writes.length, 1)
  assert.equal(JSON.stringify(h.writes[0].data), JSON.stringify({ entryId: 'entry-a', revision: 1, target: action().target }))
  h.writes[0].resolve()
  await confirmation
  assert.equal(h.state.open.value, false)
  await h.reply([action({ revision: 2, busy: true, status: 'queued' })])
  assert.equal(h.tick(), 2000)
  await h.reply([action({ revision: 5, status: 'ready', syncedAt: '2026-09-17T01:00:00Z' })])
  assert.equal(h.reloads(), 1)
  assert.ok(h.notices.some(text => text.includes('凭据已同步')))
  assert.equal(h.tick(), 15000)
  await h.reply([action({ revision: 5, status: 'ready', syncedAt: '2026-09-17T01:00:00Z' })])
  assert.equal(h.reloads(), 1)
})

test('a stale confirmation never adopts a new material revision or workspace', async (t) => {
  const h = harness(t)
  await h.reply()
  h.state.request(account)
  h.tick()
  await h.reply([action({ revision: 2, target: { ...action().target, workspace_id: 'workspace-b' } })])
  assert.equal(h.state.selected.value.action.revision, 1)
  assert.equal(h.state.selected.value.action.target.workspace_id, 'workspace-a')
  assert.equal(h.state.confirmDisabled.value, true)
  await h.state.confirm()
  assert.equal(h.writes.length, 0)
})

test('missing material, global pause and busy jobs cannot open confirmation', async (t) => {
  const h = harness(t)
  for (const items of [[], [action({ busy: true })], [action({ blockedReason: 'paused' })], [action({ target: null })]]) {
    await h.reply(items)
    h.state.request(account)
    assert.equal(h.state.open.value, false)
    h.tick()
  }
})

test('failed reads fail closed and retry; action errors survive successful polling without replay', async (t) => {
  const h = harness(t)
  await h.reply()
  h.tick()
  h.reads.at(-1).reject(new Error('offline'))
  await flush()
  h.state.request(account)
  assert.equal(h.state.open.value, false)
  assert.equal(h.tick(), 5000)
  await h.reply()
  h.state.request(account)
  const submitted = h.state.confirm()
  h.writes[0].reject(new Error('response lost'))
  await submitted
  await h.reply([action({ revision: 2, busy: true, status: 'running' })])
  assert.equal(h.state.actionError.value, 'response lost')
  assert.equal(h.state.confirmDisabled.value, true)
  h.tick()
  await h.reply([action({ revision: 3, status: 'failed', message: 'verification failed' })])
  assert.equal(h.state.actionError.value, 'response lost')
  assert.equal(h.writes.length, 1)
  assert.ok(h.notices.some(text => text.includes('verification failed')))
})

test('page changes and disposal fence stale reads and do not cancel accepted work', async (t) => {
  const h = harness(t)
  const initial = h.reads[0]
  h.accounts.value = [{ ...account, id: 'acct-b' }]
  assert.equal(initial.options.signal.aborted, true)
  await h.reply([])
  initial.resolve([action()])
  await flush()
  assert.equal(Object.keys(h.state.actions.value).length, 0)
  h.accounts.value = [account]
  await h.reply()
  h.state.request(account)
  const submit = h.state.confirm()
  h.scope.stop()
  assert.equal(h.timers.size, 0)
  const reads = h.reads.length
  h.writes[0].resolve()
  await submit
  assert.equal(h.reads.length, reads)
  assert.equal(h.timers.size, 0)
  assert.equal(h.reloads(), 0)
  assert.equal(h.writes.length, 1)
})

test('same-ID account reload keeps status baseline and does not restart reads', async (t) => {
  const h = harness(t)
  await h.reply([action({ busy: true, status: 'running' })])
  h.accounts.value = [{ ...account, name: 'renamed' }]
  await flush()
  assert.equal(h.reads.length, 1)
  h.tick()
  await h.reply([action({ revision: 3, status: 'failed', message: 'cancelled' })])
  assert.equal(h.reloads(), 1)
})

test('real menu renders relogin only with material and exposes busy and failure states', async () => {
  const passthrough = vue.defineComponent({ setup: (_, { slots }) => () => vue.h('div', slots.default?.({ close() {} })) })
  const button = vue.defineComponent({
    props: ['disabled'],
    setup: (props, { slots }) => () => vue.h('button', { disabled: props.disabled }, slots.default?.()),
  })
  const component = load(new URL('../src/views/accounts/components/AccountTableActions.vue', import.meta.url), {
    '@/components/base/BasePopover.vue': passthrough,
    '@/components/base/BaseMenuItem.vue': button,
    '@/components/base/BaseIconButton.vue': button,
  }).default
  async function render(relogin) {
    return renderToString(vue.createSSRApp(component, { account, deleting: false, recovering: false, refreshing: false, testing: false, relogin }))
  }
  assert.doesNotMatch(await render(), /失效重登|重登处理中/)
  assert.match(await render(action()), /失效重登/)
  assert.match(await render(action({ busy: true })), /<button disabled[^>]*>\s*重登处理中\s*<\/button>/)
  assert.match(await render(action({ status: 'failed', message: '<script>failed' })), /&lt;script&gt;failed/)
})
