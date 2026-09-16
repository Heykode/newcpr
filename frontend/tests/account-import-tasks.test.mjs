/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { webcrypto } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'
import { compileScript, parse } from 'vue/compiler-sfc'
import { renderToString } from 'vue/server-renderer'

const require = createRequire(import.meta.url)
const sourceRoot = new URL('../src/', import.meta.url)
const taskPath = 'views/accounts/composables/useAccountImportTasks.ts'
const onboardingPath = 'views/accounts/composables/useAccountOnboarding.ts'
const taskComponentPath = 'views/accounts/components/AccountImportTasks/'

function loader(dependencies = {}, globals = {}) {
  const modules = new Map()
  function load(path) {
    const filename = path instanceof URL ? path : new URL(path, sourceRoot)
    if (modules.has(filename.href))
      return modules.get(filename.href)
    const exports = {}
    modules.set(filename.href, exports)
    const source = readFileSync(filename, 'utf8')
    const content = filename.pathname.endsWith('.vue')
      ? compileScript(parse(source, { filename: filename.pathname }).descriptor, {
        id: filename.pathname,
        inlineTemplate: true,
      }).content
      : source
    const { outputText } = ts.transpileModule(content, {
      compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
    })
    runInNewContext(outputText, {
      exports,
      AbortController,
      crypto: webcrypto,
      setTimeout,
      clearTimeout,
      ...globals,
      require(name) {
        if (name in dependencies)
          return dependencies[name]
        if (name.startsWith('@/') || name.startsWith('.')) {
          const path = name.endsWith('.vue') ? name : `${name}.ts`
          return load(name.startsWith('@/') ? new URL(path.slice(2), sourceRoot) : new URL(path, filename))
        }
        return require(name)
      },
    }, { filename: filename.pathname })
    return exports
  }
  return load
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

function task(taskId = 'task-a', overrides = {}) {
  return {
    taskId,
    createdAt: '2026-09-16T04:00:00Z',
    finishedAt: null,
    stopRequested: false,
    total: 2,
    counts: { pending: 1, running: 1, succeeded: 0, failed: 0, unknown: 0, skipped: 0, importedAccounts: 0 },
    items: [],
    ...overrides,
  }
}

function completed(taskId = 'task-a', overrides = {}) {
  return task(taskId, {
    finishedAt: '2026-09-16T04:00:01Z',
    counts: { pending: 0, running: 0, succeeded: 2, failed: 0, unknown: 0, skipped: 0, importedAccounts: 3 },
    ...overrides,
  })
}

function messages() {
  const errors = []
  const successes = []
  return {
    errors,
    successes,
    toast: { error: text => errors.push(text), success: text => successes.push(text) },
  }
}

function mountOnboarding(t, api = {}, globals = {}) {
  const notifications = messages()
  const submissions = []
  const created = []
  const oauthStarts = []
  const oauthCompletions = []
  let reloads = 0
  const load = loader({
    '@/api': {
      async createAccountImportTask(input) {
        submissions.push(structuredClone(input))
        return api.create ? api.create(input) : task()
      },
      async startAccountOAuth(input) {
        oauthStarts.push(structuredClone(input))
        return { flowId: 'flow-a', authorizationUrl: 'https://example.com/oauth' }
      },
      async completeAccountOAuth(input) {
        oauthCompletions.push(structuredClone(input))
        return { accountId: 'account-existing' }
      },
    },
    '@/api/request': { ApiError: class ApiError extends Error {} },
    '@/components/base/BaseToast': notifications,
  }, globals)
  const scope = vue.effectScope()
  t.after(() => scope.stop())
  const state = scope.run(() => load(onboardingPath).useAccountOnboarding({
    reload: async () => { reloads++ },
    onImportTaskCreated: result => created.push(result),
  }))
  function input(provider, mode, value) {
    state.openCreateAccount()
    state.createForm.value.provider = provider
    state.createForm.value.mode = mode
    state.createForm.value.step = 'import'
    if (mode !== 'oauth')
      state.createForm.value.importTexts[mode] = value
  }
  return { state, input, submissions, created, oauthStarts, oauthCompletions, notifications, reloads: () => reloads }
}

function mountTasks(t, api = {}) {
  const mounted = []
  const timers = new Map()
  const requests = { list: [], detail: [], stop: [] }
  const notifications = messages()
  let nextTimer = 0
  let reloads = 0
  const load = loader({
    'vue': { ...vue, onMounted: fn => mounted.push(fn) },
    '@/api': {
      getAccountImportTasks(options) {
        const entry = { ...deferred(), options }
        requests.list.push(entry)
        return entry.promise
      },
      getAccountImportTask(input, options) {
        const entry = { ...deferred(), input, options }
        requests.detail.push(entry)
        return entry.promise
      },
      stopAccountImportTask(input) {
        const entry = { ...deferred(), input }
        requests.stop.push(entry)
        return entry.promise
      },
    },
    '@/api/request': { ApiError: class ApiError extends Error {} },
    '@/components/base/BaseToast': notifications,
  }, {
    setTimeout(callback, delay) {
      const id = ++nextTimer
      timers.set(id, { callback, delay })
      return id
    },
    clearTimeout: id => timers.delete(id),
  })
  const scope = vue.effectScope()
  t.after(() => scope.stop())
  const state = scope.run(() => load(taskPath).useAccountImportTasks({
    reload: async () => {
      reloads++
      await api.reload?.()
    },
  }))
  return {
    state,
    scope,
    timers,
    requests,
    notifications,
    reloads: () => reloads,
    mount: () => mounted.forEach(fn => fn()),
    async list(items) {
      requests.list.at(-1).resolve({ items })
      await flush()
    },
    async detail(result) {
      requests.detail.at(-1).resolve(result)
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

test('account task APIs match upstream methods, body/query fields and read cancellation', async () => {
  const requests = []
  const load = loader({
    '../request': async (config) => {
      requests.push(config)
      return {}
    },
  })
  const api = load('api/modules/accounts.ts')
  const signal = new AbortController().signal
  const payload = { submissionId: '11111111-1111-4111-8111-111111111111', items: [{ provider: 'openai', data: {} }] }
  await api.createAccountImportTask(payload)
  await api.getAccountImportTasks({ silent: true, signal })
  await api.getAccountImportTask({ taskId: 'task-a' }, { silent: true, signal })
  await api.stopAccountImportTask({ taskId: 'task-a' })
  await api.importAccounts({ provider: 'openai', data: { accounts: [] } })
  assert.deepEqual(requests.map(({ url, method }) => [url, method]), [
    ['/api/admin/accounts/import-tasks', 'POST'],
    ['/api/admin/accounts/import-tasks', 'GET'],
    ['/api/admin/accounts/import-tasks/detail', 'GET'],
    ['/api/admin/accounts/import-tasks/stop', 'POST'],
    ['/api/admin/accounts/import', 'POST'],
  ])
  assert.equal(requests[0].data, payload)
  assert.equal(requests[0].params, undefined)
  assert.deepEqual(requests[2].params, { taskId: 'task-a' })
  assert.deepEqual(requests[3].data, { taskId: 'task-a' })
  for (const request of requests.slice(1, 3)) {
    assert.equal(request.silent, true)
    assert.equal(request.signal, signal)
  }
})

for (const mode of ['access_token', 'refresh_token']) {
  test(`${mode} submits one document per nonempty line with existing settings and no client token loop`, async (t) => {
    const h = mountOnboarding(t)
    h.input('openai', mode, ' synthetic-a \r\n \n synthetic-b\nsynthetic-a ')
    Object.assign(h.state.createForm.value, {
      enabled: false,
      concurrencyLimit: '7',
      weight: '23',
      groupIds: ['group-a', 'group-a', 'group-b'],
      proxyMode: 'proxy',
      proxyId: ' proxy-a ',
    })
    await h.state.handleCreate()
    assert.equal(h.submissions.length, 1)
    const body = h.submissions[0]
    assert.match(body.submissionId, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/)
    const key = mode === 'access_token' ? 'accessToken' : 'refreshToken'
    assert.deepEqual(body.items, ['synthetic-a', 'synthetic-b', 'synthetic-a'].map(value => ({
      provider: 'openai',
      data: { accounts: [{ [key]: value }] },
      settings: { enabled: false, concurrencyLimit: 7, weight: 23, groupIds: ['group-a', 'group-b'] },
      outboundProxyId: 'proxy-a',
    })))
    assert.equal(h.created.length, 1)
    assert.equal(h.state.showCreateModal.value, false)
    assert.equal(h.state.createForm.value.importTexts[mode], '')
    assert.equal(h.reloads(), 0, 'creation is not proof of account persistence')
    assert.deepEqual(h.notifications.errors, [])
  })
}

test('JSON documents retain metadata, multi-account boundaries and provider filtering', async (t) => {
  const openai = { version: 1, metadata: { keep: true }, accounts: [{ accessToken: 'synthetic-a' }, { refreshToken: 'synthetic-b' }] }
  const xai = { provider: 'xai', accounts: [{ refreshToken: 'synthetic-c' }] }
  const mixed = { documents: [{ provider: 'openai', document: openai }, { provider: 'xai', document: xai }] }
  for (const [provider, input, documents] of [
    ['openai', openai, [openai]],
    ['xai', xai, [xai]],
    ['openai', mixed, [openai]],
    ['xai', mixed, [xai]],
    ['batch', mixed, [openai, xai]],
  ]) {
    const h = mountOnboarding(t)
    h.input(provider, 'json', JSON.stringify(input))
    await h.state.handleCreate()
    assert.equal(h.submissions.length, 1)
    assert.deepEqual(h.submissions[0].items.map(entry => entry.data), documents)
    assert.deepEqual(h.submissions[0].items.map(entry => entry.provider), provider === 'batch' ? ['openai', 'xai'] : [provider])
    assert.ok(h.submissions[0].items.every(entry => entry.outboundProxyId === undefined))
  }
})

test('double submit is blocked and an uncertain POST keeps the same ID until form changes or closes', async (t) => {
  const replies = []
  const h = mountOnboarding(t, { create: () => {
    const reply = deferred()
    replies.push(reply)
    return reply.promise
  } })
  h.input('openai', 'refresh_token', 'synthetic-a')
  const first = h.state.handleCreate()
  await h.state.handleCreate()
  assert.equal(h.submissions.length, 1)
  assert.equal(h.state.creatingAccount.value, true)
  replies[0].reject(new Error('response lost'))
  await first
  assert.equal(h.state.showCreateModal.value, true)
  assert.equal(h.state.creatingAccount.value, false)
  await flush()
  assert.equal(h.submissions.length, 1, 'no automatic token replay after timeout')
  const retry = h.state.handleCreate()
  assert.equal(h.submissions[1].submissionId, h.submissions[0].submissionId)
  replies[1].reject(new Error('still unavailable'))
  await retry
  h.state.createForm.value.groupIds.push('group-new')
  const changed = h.state.handleCreate()
  assert.notEqual(h.submissions[2].submissionId, h.submissions[1].submissionId)
  replies[2].reject(new Error('unavailable'))
  await changed
  h.state.showCreateModal.value = false
  h.input('openai', 'refresh_token', 'synthetic-a')
  const reopened = h.state.handleCreate()
  assert.notEqual(h.submissions[3].submissionId, h.submissions[2].submissionId)
  replies[3].resolve(task())
  await reopened
  assert.equal(h.created.length, 1)
})

test('uncertain token submission survives back/continue, normalized whitespace and unused form changes on HTTP', async (t) => {
  const storage = new Proxy({}, {
    get() {
      throw new Error('must not persist submission material')
    },
  })
  const h = mountOnboarding(t, {
    create: async () => {
      throw new Error('accepted response lost')
    },
  }, {
    crypto: { getRandomValues: bytes => webcrypto.getRandomValues(bytes) },
    localStorage: storage,
    sessionStorage: storage,
  })
  h.input('openai', 'refresh_token', 'synthetic-a\nsynthetic-b')
  Object.assign(h.state.createForm.value, { groupIds: ['group-a'], concurrencyLimit: '2', weight: '3' })
  await h.state.handleCreate()
  const original = h.submissions[0]
  const changes = [
    (form) => {
      form.step = 'settings'
    },
    (form) => {
      form.step = 'import'
    },
    (form) => {
      form.importTexts.refresh_token = '\n synthetic-a \r\n \n synthetic-b \n'
    },
    (form) => {
      form.importTexts.access_token = 'unused-synthetic-token'
    },
    (form) => {
      form.importTexts.json = '{invalid unused JSON'
    },
    (form) => {
      form.oauthCallback = 'unused-callback'
    },
    (form) => {
      form.proxyId = 'unused-proxy'
    },
    (form) => {
      form.weight = ' 03 '
    },
    (form) => {
      form.concurrencyLimit = ' 02 '
    },
    (form) => {
      form.groupIds.push('group-a')
    },
  ]
  for (const change of changes) {
    change(h.state.createForm.value)
    await h.state.handleCreate()
    assert.deepEqual(h.submissions.at(-1), original)
  }
  h.state.createForm.value.weight = 'invalid'
  const count = h.submissions.length
  await h.state.handleCreate()
  assert.equal(h.submissions.length, count)
  h.state.createForm.value.weight = '3'
  await h.state.handleCreate()
  assert.deepEqual(h.submissions.at(-1), original, 'temporary invalid edits cannot discard uncertain submission identity')
  assert.equal(h.created.length, 0)
})

test('semantic JSON retries preserve IDs through formatting, key order, mode and filtered documents', async (t) => {
  const h = mountOnboarding(t, {
    create: async () => {
      throw new Error('response lost')
    },
  })
  h.input('openai', 'json', '{"meta":{"a":1,"b":2},"accounts":[{"refreshToken":"synthetic-a"}]}')
  await h.state.handleCreate()
  const first = h.submissions[0]
  h.state.createForm.value.importTexts.json = `{
    "accounts": [{"refreshToken": "synthetic-a"}],
    "meta": {"b": 2, "a": 1}
  }`
  await h.state.handleCreate()
  assert.deepEqual(h.submissions.at(-1), first)
  h.state.createForm.value.importTexts.json = JSON.stringify({
    documents: [
      { provider: 'xai', document: { accounts: [{ refreshToken: 'unused-synthetic-token' }] } },
      { provider: 'openai', document: { meta: { b: 2, a: 1 }, accounts: [{ refreshToken: 'synthetic-a' }] } },
    ],
  })
  await h.state.handleCreate()
  assert.deepEqual(h.submissions.at(-1), first, 'unselected providers do not affect the effective request')

  h.input('openai', 'refresh_token', 'synthetic-a')
  await h.state.handleCreate()
  const token = h.submissions.at(-1)
  h.state.createForm.value.mode = 'json'
  h.state.createForm.value.importTexts.json = '{"accounts":[{"refreshToken":"synthetic-a"}]}'
  await h.state.handleCreate()
  assert.deepEqual(h.submissions.at(-1), token, 'mode changes alone do not change an equivalent wire payload')
})

test('every effective payload change renews the submission ID while proxy trimming does not', async (t) => {
  const h = mountOnboarding(t, {
    create: async () => {
      throw new Error('response lost')
    },
  })
  h.input('openai', 'refresh_token', 'synthetic-a')
  await h.state.handleCreate()
  const changes = [
    (form) => {
      form.importTexts.refresh_token = 'synthetic-b'
    },
    (form) => {
      form.enabled = false
    },
    (form) => {
      form.weight = '7'
    },
    (form) => {
      form.concurrencyLimit = '4'
    },
    (form) => {
      form.groupIds = ['group-a']
    },
    (form) => {
      Object.assign(form, { proxyMode: 'proxy', proxyId: 'proxy-a' })
    },
    (form) => {
      form.proxyId = 'proxy-b'
    },
    (form) => {
      form.proxyMode = 'direct'
    },
    (form) => {
      form.mode = 'json'
      form.importTexts.json = '{"accounts":[{"refreshToken":"synthetic-b"}],"metadata":"new"}'
    },
    (form) => {
      form.provider = 'xai'
      // Provider changes replace the form through its synchronous watcher.
      h.state.createForm.value.mode = 'json'
      h.state.createForm.value.importTexts.json = '{"accounts":[{"refreshToken":"synthetic-b"}],"metadata":"new"}'
    },
  ]
  for (const change of changes) {
    const before = h.submissions.at(-1).submissionId
    change(h.state.createForm.value)
    await h.state.handleCreate()
    assert.notEqual(h.submissions.at(-1).submissionId, before)
  }
  Object.assign(h.state.createForm.value, { proxyMode: 'proxy', proxyId: 'proxy-a' })
  await h.state.handleCreate()
  const proxy = h.submissions.at(-1)
  h.state.createForm.value.proxyId = ' proxy-a '
  await h.state.handleCreate()
  assert.deepEqual(h.submissions.at(-1), proxy)
})

test('invalid input never submits and the 200-item boundary counts documents, not JSON accounts', async (t) => {
  const invalid = [
    ['openai', 'access_token', ' \n '],
    ['openai', 'refresh_token', Array.from({ length: 201 }).fill('synthetic-a').join('\n')],
    ['openai', 'json', '{'],
    ['openai', 'json', 'null'],
    ['openai', 'json', '[]'],
    ['batch', 'json', '{"accounts":[]}'],
    ['batch', 'json', '{"documents":[]}'],
    ['batch', 'json', '{"documents":[{"provider":"other","document":{}}]}'],
    ['batch', 'json', '{"documents":[{"provider":"xai","document":null}]}'],
    ['openai', 'json', '{"documents":[{"provider":"xai","document":{}}]}'],
    ['batch', 'json', JSON.stringify({ documents: Array.from({ length: 201 }, () => ({ provider: 'xai', document: {} })) })],
  ]
  for (const [provider, mode, input] of invalid) {
    const h = mountOnboarding(t)
    h.input(provider, mode, input)
    await h.state.handleCreate()
    assert.equal(h.submissions.length, 0, `${provider} ${mode}`)
    assert.equal(h.notifications.errors.length, 1)
    assert.equal(h.state.showCreateModal.value, true)
  }
  const h = mountOnboarding(t)
  h.input('openai', 'access_token', Array.from({ length: 200 }).fill('synthetic-a').join('\n'))
  await h.state.handleCreate()
  assert.equal(h.submissions[0].items.length, 200)
  h.input('openai', 'json', JSON.stringify({ accounts: Array.from({ length: 201 }, () => ({ accessToken: 'synthetic-a' })) }))
  await h.state.handleCreate()
  assert.equal(h.submissions[1].items.length, 1, 'legacy JSON validation remains server-owned')
  for (const values of [{ proxyMode: 'proxy', proxyId: '' }, { weight: 'invalid' }]) {
    h.input('openai', 'access_token', 'synthetic-a')
    Object.assign(h.state.createForm.value, values)
    await h.state.handleCreate()
    assert.equal(h.submissions.length, 2)
  }
})

test('OAuth creation and existing-account relogin keep their original APIs and settings semantics', async (t) => {
  for (const provider of ['openai', 'xai']) {
    const h = mountOnboarding(t)
    h.input(provider, 'oauth')
    await h.state.handleAuthorizeOAuth()
    h.state.createForm.value.oauthCallback = 'https://example.com/callback?code=synthetic-code'
    await h.state.handleCreate()
    assert.equal(h.submissions.length, 0)
    assert.equal(h.oauthStarts[0].accountId, undefined)
    assert.deepEqual(h.oauthCompletions[0].settings, { enabled: true, concurrencyLimit: null, weight: 1, groupIds: [] })
    assert.equal(h.reloads(), 1)

    h.state.openReauthorizeAccount({ id: 'account-existing', provider, name: 'Existing', enabled: false })
    await flush()
    assert.equal(h.oauthStarts[1].accountId, 'account-existing')
    assert.equal(h.oauthStarts[1].outboundProxyId, undefined)
    h.state.createForm.value.oauthCallback = 'synthetic-authorization-code'
    await h.state.handleCreate()
    assert.equal(h.oauthCompletions[1].settings, undefined)
    assert.equal(h.submissions.length, 0)
    assert.equal(h.reloads(), 2)
  }
})

test('mount restores server jobs; reopening discovers terminal/new tasks without reimporting', async (t) => {
  const h = mountTasks(t)
  h.mount()
  assert.equal(h.requests.list.length, 1)
  await h.list([completed()])
  assert.equal(h.state.tasks.value.length, 1)
  assert.equal(h.reloads(), 0)
  assert.equal(h.timers.size, 0)
  h.state.open.value = true
  await flush()
  await h.list([task('task-b'), completed()])
  assert.equal(h.requests.detail.at(-1).input.taskId, 'task-a', 'retain the existing selection')
  await h.detail(completed())
  assert.equal(h.timers.size, 1, 'even all-terminal open panels continue discovery')
  h.state.select('task-b')
  await h.list([task('task-b'), completed()])
  await h.detail(task('task-b'))
  assert.equal(h.state.detail.value.taskId, 'task-b')
  h.state.open.value = false
  await flush()
  const detailReads = h.requests.detail.length
  await h.list([completed('task-b'), completed()])
  assert.equal(h.requests.detail.length, detailReads, 'closed panels only read summaries')
  assert.equal(h.reloads(), 1)
  assert.equal(h.timers.size, 0)
  h.state.open.value = true
  await flush()
  await h.list([])
  assert.equal(h.state.selectedId.value, '')
  assert.equal(h.state.detail.value, null, 'expired or restart-lost records leave no stale detail')
  assert.equal(h.timers.size, 1)
  assert.deepEqual(h.requests.stop, [])
})

test('only newly observed imported counts refresh account/group data, including fast created tasks', async (t) => {
  const h = mountTasks(t, { reload: async () => {
    throw new Error('list unavailable')
  } })
  h.mount()
  await h.list([task()])
  assert.equal(h.reloads(), 0)
  assert.equal(h.tick(), 1500)
  await h.list([completed()])
  assert.equal(h.reloads(), 1)
  const refresh = h.state.refresh()
  await h.list([completed()])
  await refresh
  assert.equal(h.reloads(), 1, 'unchanged counts do not repeat a reload')
})

test('creation de-duplicates server task IDs and refreshes already-imported results once', async (t) => {
  const h = mountTasks(t)
  h.state.created(completed())
  await flush()
  assert.equal(h.reloads(), 1)
  assert.equal(h.state.open.value, true)
  await h.list([completed()])
  await h.detail(completed())
  h.state.created(completed())
  await h.list([completed()])
  await h.detail(completed())
  assert.equal(h.state.tasks.value.length, 1)
  assert.equal(h.reloads(), 1)
  assert.equal(h.tick(), 1500)
  await h.list([completed()])
  await h.detail(completed())
  assert.equal(h.reloads(), 1)
})

test('a created pending task establishes its baseline before the initial server query', async (t) => {
  const h = mountTasks(t)
  h.state.created(task())
  await flush()
  await h.list([completed()])
  await h.detail(completed())
  assert.equal(h.reloads(), 1)
})

test('failed reads retain data, back off, recover, and never replay a failed or unknown token', async (t) => {
  const h = mountTasks(t)
  h.mount()
  h.requests.list[0].reject(new Error('offline'))
  await flush()
  assert.equal(h.state.error.value, 'offline')
  assert.equal(h.state.loading.value, false)
  assert.equal(h.tick(), 5000, 'initial closed-panel failure must still discover jobs after reconnect')
  const unknown = completed('task-a', {
    counts: { pending: 0, running: 0, succeeded: 0, failed: 1, unknown: 1, skipped: 0, importedAccounts: 0 },
  })
  await h.list([unknown])
  assert.equal(h.state.error.value, '')
  h.state.open.value = true
  await flush()
  await h.list([unknown])
  await h.detail(unknown)
  h.tick()
  h.requests.list.at(-1).reject(new Error('read failed'))
  await flush()
  assert.equal(h.state.tasks.value[0], unknown)
  assert.equal(h.state.detail.value, unknown)
  assert.equal(h.tick(), 5000)
  await h.list([unknown])
  h.requests.detail.at(-1).reject(new Error('detail failed'))
  await flush()
  assert.equal(h.state.error.value, 'detail failed')
  assert.equal(h.state.detail.value, unknown)
  assert.equal(h.tick(), 5000)
  await h.list([unknown])
  await h.detail(unknown)
  assert.equal(h.state.error.value, '')
  assert.deepEqual(h.requests.stop, [])
  assert.equal(h.reloads(), 0)
})

test('superseded details, close and unmount cannot publish stale results or restart polling', async (t) => {
  const h = mountTasks(t)
  h.state.open.value = true
  await flush()
  await h.list([task(), task('task-b')])
  const older = h.requests.detail.at(-1)
  h.state.select('task-b')
  assert.equal(older.options.signal.aborted, true)
  await h.list([task(), task('task-b')])
  await h.detail(task('task-b'))
  older.resolve(task())
  await flush()
  assert.equal(h.state.detail.value.taskId, 'task-b')
  h.tick()
  const oldList = h.requests.list.at(-1)
  h.state.open.value = false
  await flush()
  assert.equal(oldList.options.signal.aborted, true)
  oldList.reject(new Error('late failure'))
  await h.list([task(), task('task-b')])
  assert.equal(h.state.error.value, '')
  h.tick()
  const pending = h.requests.list.at(-1)
  h.scope.stop()
  assert.equal(pending.options.signal.aborted, true)
  assert.equal(h.timers.size, 0)
  pending.resolve({ items: [completed()] })
  await flush()
  assert.equal(h.state.tasks.value.length, 2)
  assert.equal(h.reloads(), 0)
  assert.equal(h.state.loading.value, false)
  assert.equal(h.timers.size, 0)
  const count = h.requests.list.length
  await h.state.refresh()
  assert.equal(h.requests.list.length, count)
})

test('stop only targets queued items, blocks double clicks and preserves errors through polling', async (t) => {
  const h = mountTasks(t)
  h.state.open.value = true
  await flush()
  await h.list([task()])
  await h.detail(task())
  const stopped = h.state.stop()
  await h.state.stop()
  assert.equal(h.requests.stop.length, 1)
  assert.equal(h.requests.stop[0].input.taskId, 'task-a')
  h.requests.stop[0].reject(new Error('stop unavailable'))
  await stopped
  assert.equal(h.state.stopping.value, false)
  assert.equal(h.state.stopError.value, 'stop unavailable')
  h.tick()
  await h.list([task()])
  await h.detail(task())
  assert.equal(h.state.stopError.value, 'stop unavailable', 'successful reads do not erase failed actions')
  const retry = h.state.stop()
  const stopping = task('task-a', { stopRequested: true })
  h.requests.stop[1].resolve(stopping)
  await flush()
  await h.list([stopping])
  await h.detail(stopping)
  await retry
  assert.equal(h.state.stopError.value, '')
  for (const nonStoppable of [
    stopping,
    completed(),
    task('task-a', { counts: { ...task().counts, pending: 0 } }),
    null,
  ]) {
    h.state.detail.value = nonStoppable
    await h.state.stop()
    assert.equal(h.requests.stop.length, 2)
  }
})

test('late stop responses cannot replace the selected task or revive a disposed page', async (t) => {
  const h = mountTasks(t)
  h.state.open.value = true
  await flush()
  await h.list([task(), task('task-b')])
  await h.detail(task())
  const stopping = h.state.stop()
  h.state.select('task-b')
  await h.list([task(), task('task-b')])
  await h.detail(task('task-b'))
  h.requests.stop[0].reject(new Error('old task stop failed'))
  await stopping
  assert.equal(h.state.detail.value.taskId, 'task-b')
  assert.equal(h.state.stopError.value, '')
  const next = h.state.stop()
  h.scope.stop()
  const count = h.requests.list.length
  h.requests.stop[1].resolve(completed('task-b'))
  await next
  assert.equal(h.requests.list.length, count)
  assert.equal(h.timers.size, 0)
})

for (const confirmed of [
  task('task-a', { stopRequested: true }),
  completed(),
]) {
  test(`lost stop response converges after authoritative ${confirmed.stopRequested ? 'stopRequested' : 'terminal'} detail`, async (t) => {
    const h = mountTasks(t)
    h.state.open.value = true
    await flush()
    await h.list([task()])
    await h.detail(task())
    const stopping = h.state.stop()
    h.requests.stop[0].reject(new Error('stop response lost'))
    await stopping
    assert.equal(h.state.stopError.value, 'stop response lost')
    h.tick()
    await h.list([confirmed])
    assert.equal(h.state.stopError.value, 'stop response lost', 'a summary alone does not confirm selected detail')
    await h.detail(confirmed)
    assert.equal(h.state.stopError.value, '')
    assert.equal(h.requests.stop.length, 1, 'read convergence does not resend stop')
    await h.state.stop()
    assert.equal(h.requests.stop.length, 1)
  })

  test(`confirmed ${confirmed.stopRequested ? 'stopRequested' : 'terminal'} detail suppresses a later lost-stop error`, async (t) => {
    const h = mountTasks(t)
    h.state.open.value = true
    await flush()
    await h.list([task()])
    await h.detail(task())
    const stopping = h.state.stop()
    h.tick()
    await h.list([confirmed])
    await h.detail(confirmed)
    h.requests.stop[0].reject(new Error('late stop response lost'))
    await stopping
    assert.equal(h.state.stopError.value, '')
    assert.equal(h.state.detail.value, confirmed)
  })
}

test('stale or wrong-task confirmations cannot clear the selected task stop error', async (t) => {
  const h = mountTasks(t)
  h.state.open.value = true
  await flush()
  await h.list([task(), task('task-b')])
  await h.detail(task())
  h.tick()
  await h.list([task(), task('task-b')])
  const staleDetail = h.requests.detail.at(-1)
  h.state.select('task-b')
  assert.equal(staleDetail.options.signal.aborted, true)
  await h.list([task(), task('task-b')])
  await h.detail(task('task-b'))
  const stopping = h.state.stop()
  h.requests.stop[0].reject(new Error('task-b stop failed'))
  await stopping
  staleDetail.resolve(completed())
  await flush()
  assert.equal(h.state.detail.value.taskId, 'task-b')
  assert.equal(h.state.stopError.value, 'task-b stop failed')
  h.tick()
  await h.list([task(), task('task-b')])
  await h.detail(completed())
  assert.equal(h.state.detail.value.taskId, 'task-b', 'wrong-task response must not publish')
  assert.equal(h.state.stopError.value, 'task-b stop failed')
  h.tick()
  await h.list([task(), task('task-b')])
  await h.detail(task('task-b'))
  assert.equal(h.state.stopError.value, 'task-b stop failed', 'unconfirmed successful reads still retain the action error')
})

for (const outcome of ['success', 'failure']) {
  test(`stop ${outcome} from an old A-to-B-to-A selection cannot mutate the new selection`, async (t) => {
    const h = mountTasks(t)
    h.state.open.value = true
    await flush()
    await h.list([task(), task('task-b')])
    await h.detail(task())
    const stopping = h.state.stop()
    h.state.select('task-b')
    await h.list([task(), task('task-b')])
    await h.detail(task('task-b'))
    h.state.select('task-a')
    await h.list([task(), task('task-b')])
    const selected = task()
    await h.detail(selected)
    const reads = h.requests.list.length
    if (outcome === 'success')
      h.requests.stop[0].resolve(completed())
    else
      h.requests.stop[0].reject(new Error('old selection stop failed'))
    await stopping
    assert.equal(h.state.detail.value, selected)
    assert.equal(h.state.stopError.value, '')
    assert.equal(h.requests.list.length, reads, 'stale actions must not supersede current polling')
  })
}

function componentLoader() {
  const passthrough = tag => vue.defineComponent({
    setup: (_, { slots }) => () => vue.h(tag, [slots.default?.(), slots.footer?.()]),
  })
  const button = vue.defineComponent({
    props: ['disabled', 'loading'],
    setup: (props, { slots }) => () => vue.h('button', { disabled: props.disabled || props.loading }, slots.default?.()),
  })
  const empty = vue.defineComponent({
    props: ['title'],
    setup: props => () => vue.h('p', props.title),
  })
  const icons = Object.fromEntries(['ArrowUpRight', 'Check', 'CircleAlert', 'Square', 'X', 'ListTodo']
    .map(name => [name, vue.defineComponent({ setup: () => () => vue.h('svg', { 'data-icon': name }) })]))
  return loader({
    '@lucide/vue': icons,
    '@/components/base/BaseButton.vue': button,
    '@/components/base/BaseModal/index.vue': passthrough('section'),
    '@/components/base/BaseScrollbar.vue': passthrough('div'),
    '@/components/base/BaseSegmented.vue': passthrough('nav'),
    '@/components/base/BaseSelect.vue': passthrough('select'),
    '@/components/base/BaseEmpty.vue': empty,
    '@/components/ProviderIconGroup.vue': passthrough('i'),
  })
}

test('real task SFCs render all outcomes, safe errors, accessible progress and no credential retry', async () => {
  const load = componentLoader()
  const component = load(`${taskComponentPath}TaskDetail.vue`).default
  const summary = task('task-a', {
    total: 6,
    counts: { pending: 1, running: 1, succeeded: 1, failed: 1, unknown: 1, skipped: 1, importedAccounts: 3 },
    items: ['pending', 'running', 'succeeded', 'failed', 'unknown', 'skipped'].map((status, index) => ({
      index: index + 1,
      provider: 'openai',
      status,
      accountIds: [],
      message: status === 'failed' ? 'Error <script> & detail.' : null,
    })),
  })
  const html = await renderToString(vue.createSSRApp(component, { task: summary, stopping: false }))
  for (const label of ['等待中', '处理中', '已入库', '失败', '待核对', '未执行', '不要直接重试原 Token', '停止未开始条目', '查看账号'])
    assert.ok(html.includes(label), label)
  assert.match(html, /role="progressbar"[^>]*aria-label="条目处理进度"[^>]*aria-valuenow="4"[^>]*aria-valuemax="6"/)
  assert.match(html, /已入库 3 个账号/)
  assert.match(html, /Error &lt;script&gt; &amp; detail/)
  assert.doesNotMatch(html, /<script>|重试失败条目|重新导入<\/button>/)
  const waiting = await renderToString(vue.createSSRApp(component, {
    task: task('task-a', { counts: { ...task().counts, pending: 0 } }),
    stopping: false,
  }))
  assert.match(waiting, /<button disabled[^>]*>\s*等待当前条目结束<\/button>/)
  const done = await renderToString(vue.createSSRApp(component, { task: completed(), stopping: false }))
  assert.doesNotMatch(done, /停止未开始条目|等待当前条目结束/)
  const zero = await renderToString(vue.createSSRApp(component, {
    task: task('task-a', { total: 0, counts: { pending: 0, running: 0, succeeded: 0, failed: 0, unknown: 0, skipped: 0, importedAccounts: 0 } }),
    stopping: false,
  }))
  assert.doesNotMatch(zero, /NaN|Infinity/)

  const panel = load(`${taskComponentPath}index.vue`).default
  const panelHtml = await renderToString(vue.createSSRApp(panel, {
    modelValue: true,
    tasks: [summary],
    selectedId: 'task-a',
    detail: summary,
    loading: false,
    stopping: false,
    error: 'offline <error>',
    stopError: 'stop <failed>',
  }))
  assert.match(panelHtml, /role="alert"/)
  assert.match(panelHtml, /offline &lt;error&gt;/)
  assert.match(panelHtml, /stop &lt;failed&gt;/)
  assert.match(panelHtml, /刷新进度/)
  assert.doesNotMatch(panelHtml, /<error>|<failed>/)
})

test('task presenters distinguish stopping, terminal attention and per-document account counts', () => {
  const presenter = loader()(`${taskComponentPath}presenter.ts`)
  assert.equal(presenter.taskLabel(task('task-a', { counts: { ...task().counts, running: 0 } })), '排队中')
  assert.equal(presenter.taskLabel(task()), '导入中')
  assert.equal(presenter.taskLabel(task('task-a', { stopRequested: true })), '正在停止')
  assert.equal(presenter.taskLabel(completed('task-a', { stopRequested: true })), '已停止')
  assert.equal(presenter.taskLabel(completed()), '全部完成')
  assert.equal(presenter.taskLabel(completed('task-a', { counts: { ...completed().counts, unknown: 1 } })), '已结束 · 有待处理项')
  assert.equal(presenter.itemDescription({ status: 'succeeded', message: null, accountIds: ['account-a', 'account-b'] }), '已保存 2 个账号')
  assert.equal(presenter.itemDescription({ status: 'unknown', message: null, accountIds: [] }), '请检查账号列表，确认是否已入库')
})
