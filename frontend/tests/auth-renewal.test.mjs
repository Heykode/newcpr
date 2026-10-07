/* eslint-disable test/no-import-node-test -- Node's built-in runner matches existing tests. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import axios from 'axios'
import * as pinia from 'pinia'
import ts from 'typescript'
import * as vue from 'vue'

const require = createRequire(import.meta.url)

function load(path, dependencies = {}) {
  const source = readFileSync(new URL(path, import.meta.url), 'utf8')
  const { outputText } = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  const exports = {}
  runInNewContext(outputText, {
    exports,
    setTimeout,
    require: name => dependencies[name] ?? require(name),
  })
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

function requestHarness(adapter) {
  let renewals = 0
  let logouts = 0
  const messages = []
  const module = load('../src/api/request.ts', {
    'axios': { ...axios, create: config => axios.create({ ...config, adapter }) },
    './constants': { API_BASE_URL: '', API_TIMEOUT_MS: 1000 },
    './error': load('../src/api/error.ts'),
    '@/components/base/BaseToast': { toast: { error: message => messages.push(message) } },
  })
  module.setUnauthorizedHandler(() => {
    logouts += 1
  })
  module.setSessionRecoveryHandler(async () => {
    renewals += 1
    await new Promise(resolve => setTimeout(resolve, 5))
    return true
  })
  return { module, messages, renewals: () => renewals, logouts: () => logouts }
}

function response(config, status, code = 200) {
  return { config, status, statusText: '', headers: {}, data: { code, message: 'test', data: { ok: true } } }
}

test('parallel expired requests share one renewal and replay once', async () => {
  const calls = []
  const app = requestHarness(async (config) => {
    calls.push(config)
    return config.authRetried ? response(config, 200) : response(config, 401, 40101)
  })
  const results = await Promise.all(Array.from({ length: 8 }, () => app.module.default({ url: '/api/admin/accounts' })))
  assert.ok(results.every(result => result.ok))
  assert.equal(app.renewals(), 1)
  assert.equal(app.logouts(), 0)
  assert.equal(calls.length, 16)
})

test('failed recovery logs out once, while login errors do not trigger recovery', async () => {
  const app = requestHarness(async config => response(config, 401, 40101))
  await assert.rejects(app.module.default({ url: '/api/admin/accounts', silent: true }))
  assert.equal(app.renewals(), 1)
  assert.equal(app.logouts(), 1)
  await assert.rejects(app.module.default({ url: '/api/admin/auth/login', method: 'POST', silent: true }))
  assert.equal(app.renewals(), 1)
})

test('temporary errors retry reads once but never replay writes', async () => {
  const calls = []
  const app = requestHarness(async (config) => {
    calls.push(config.method)
    throw new axios.AxiosError('network down', 'ERR_NETWORK', config)
  })
  await assert.rejects(app.module.default({ url: '/api/admin/accounts', method: 'GET', silent: true }))
  await assert.rejects(app.module.default({ url: '/api/admin/accounts', method: 'POST', silent: true }))
  assert.deepEqual(calls, ['get', 'get', 'post'])
})

test('old request failure cannot invalidate a new login generation', async () => {
  const pending = deferred()
  const app = requestHarness(async config => response(config, await pending.promise, 40101))
  const request = app.module.default({ url: '/api/admin/accounts' })
  await new Promise(resolve => setTimeout(resolve, 0))
  app.module.invalidatePendingRequests()
  pending.resolve(401)
  await assert.rejects(request)
  assert.equal(app.renewals(), 0)
  assert.equal(app.logouts(), 0)
  assert.equal(app.messages.length, 0)
})

test('transient renewal failure preserves authentication', async () => {
  const app = requestHarness(async config => response(config, 401, 40101))
  app.module.setSessionRecoveryHandler(async () => {
    throw new Error('offline')
  })
  await assert.rejects(app.module.default({ url: '/api/admin/accounts' }), /offline/)
  assert.equal(app.logouts(), 0)
})

test('login waits for older refresh cookie and logout before setting the new cookie', async () => {
  const refresh = deferred()
  const logout = deferred()
  const calls = []
  const module = load('../src/stores/modules/auth.ts', {
    pinia,
    vue,
    '@/api': {
      refreshAuthSession: () => {
        calls.push('refresh')
        return refresh.promise
      },
      logout: () => {
        calls.push('logout')
        return logout.promise
      },
      login: async () => { calls.push('login') },
    },
    '@/api/request': { ...load('../src/api/error.ts'), invalidatePendingRequests: () => {}, resetUnauthorizedHandling: () => {} },
  })
  const store = module.useAuthStore(pinia.createPinia())
  const checking = store.checkAuth()
  const leaving = store.logout()
  const entering = store.login({ password: 'test' })
  assert.deepEqual(calls, ['refresh'])
  refresh.resolve({ authenticated: true })
  await checking
  await new Promise(resolve => setTimeout(resolve, 0))
  assert.deepEqual(calls, ['refresh', 'logout'])
  assert.equal(store.isAuthenticated, false)
  logout.resolve()
  await leaving
  assert.equal(await entering, true)
  assert.deepEqual(calls, ['refresh', 'logout', 'login'])
  assert.equal(store.isAuthenticated, true)
  assert.equal(store.loading, false)
})

test('explicit invalidation cannot hide an older cookie refresh from a new login', async () => {
  const refresh = deferred()
  const calls = []
  const module = load('../src/stores/modules/auth.ts', {
    pinia,
    vue,
    '@/api': {
      refreshAuthSession: () => refresh.promise,
      login: async () => { calls.push('login') },
    },
    '@/api/request': { ...load('../src/api/error.ts'), invalidatePendingRequests: () => {}, resetUnauthorizedHandling: () => {} },
  })
  const store = module.useAuthStore(pinia.createPinia())
  const checking = store.checkAuth()
  store.invalidateSession()
  const entering = store.login({ password: 'synthetic-password' })
  await new Promise(resolve => setTimeout(resolve, 0))
  assert.deepEqual(calls, [])
  refresh.resolve({ authenticated: true })
  await checking
  assert.equal(await entering, true)
  assert.deepEqual(calls, ['login'])
})

test('password changes recover expired sessions without treating them as login requests', async () => {
  const app = requestHarness(async config => config.authRetried ? response(config, 200) : response(config, 401, 40101))
  assert.equal((await app.module.default({ url: '/api/admin/auth/password', method: 'POST' })).ok, true)
  assert.equal(app.renewals(), 1)
})
