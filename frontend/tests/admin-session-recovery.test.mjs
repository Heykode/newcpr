/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import * as pinia from 'pinia'
import ts from 'typescript'
import * as vue from 'vue'
import { compileScript, parse } from 'vue/compiler-sfc'

const source = path => new URL(`../src/${path}`, import.meta.url)
const tick = () => new Promise(resolve => setImmediate(resolve))

function loadText(text, dependencies = {}, globals = {}) {
  const exports = {}
  const { outputText } = ts.transpileModule(text, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, {
    exports,
    require: (name) => {
      assert.ok(name in dependencies, `unexpected dependency: ${name}`)
      return dependencies[name]
    },
    ...globals,
  })
  return exports
}

const load = (path, dependencies) => loadText(readFileSync(source(path), 'utf8'), dependencies)
const { ApiError } = load('api/error.ts')

function deferred() {
  let resolve
  let reject
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}

function authHarness() {
  let getStatus = async () => ({ authenticated: true })
  let requests = 0
  const module = load('stores/modules/auth.ts', {
    pinia,
    vue,
    '@/api': {
      refreshAuthSession: () => {
        requests += 1
        return getStatus()
      },
      login: async () => ({}),
      logout: async () => ({}),
    },
    '@/api/request': { ApiError, invalidatePendingRequests: () => {}, resetUnauthorizedHandling: () => {} },
  })
  return {
    store: module.useAuthStore(pinia.createPinia()),
    status: (handler) => { getStatus = handler },
    requests: () => requests,
  }
}

function routeGuard(store) {
  let guard
  load('router/index.ts', {
    'vue-router': {
      createRouter: () => ({ beforeEach: (value) => { guard = value } }),
      createWebHistory: () => ({}),
    },
    '@/stores/modules/auth': { useAuthStore: () => store },
    './routes': { routes: [] },
  })
  return guard
}

test('valid sessions restore on reload and definitive failures require login', async () => {
  for (const failure of [false, new ApiError('expired', 401, 40101)]) {
    const h = authHarness()
    assert.equal(await h.store.checkAuth(), 'authenticated')
    assert.equal(h.store.isAuthenticated, true)
    h.status(async () => {
      if (failure instanceof ApiError)
        throw failure
      return { authenticated: false }
    })
    assert.equal(await h.store.checkAuth(), 'unauthenticated')
    assert.equal(h.store.isAuthenticated, false)
    assert.equal(h.store.sessionChecked, true)
    assert.equal(h.store.sessionCheckError, '')
  }
})

test('network, timeout, server and malformed responses stay recoverable', async () => {
  for (const result of [
    new ApiError('offline', 0, undefined, undefined, 'network'),
    new ApiError('timeout', 0, undefined, undefined, 'timeout'),
    new ApiError('server error', 500),
    new ApiError('gateway error', 502),
    new ApiError('unavailable', 503),
    {},
  ]) {
    const h = authHarness()
    h.status(async () => {
      if (result instanceof ApiError)
        throw result
      return result
    })
    assert.equal(await h.store.checkAuth(), 'unavailable')
    assert.equal(h.store.isAuthenticated, false)
    assert.equal(h.store.sessionChecked, false)
    assert.notEqual(h.store.sessionCheckError, '')
    h.status(async () => ({ authenticated: true }))
    assert.equal(await h.store.checkAuth(), 'authenticated')
    assert.equal(h.store.sessionChecked, true)
    assert.equal(h.store.sessionCheckError, '')
  }
})

test('a transient check does not discard an already authenticated session', async () => {
  const h = authHarness()
  await h.store.checkAuth()
  h.status(async () => {
    throw new ApiError('offline', 0)
  })
  assert.equal(await h.store.checkAuth(), 'unavailable')
  assert.equal(h.store.isAuthenticated, true)
})

test('concurrent authentication checks use one request', async () => {
  const h = authHarness()
  const response = deferred()
  h.status(() => response.promise)
  const first = h.store.checkAuth()
  const second = h.store.checkAuth()
  assert.equal(h.requests(), 1)
  response.resolve({ authenticated: true })
  assert.deepEqual(await Promise.all([first, second]), ['authenticated', 'authenticated'])
})

test('old checks cannot override login, logout or explicit invalidation', async () => {
  for (const action of ['login', 'logout', 'invalidateSession']) {
    const h = authHarness()
    const response = deferred()
    h.status(() => response.promise)
    const pending = h.store.checkAuth()
    const transition = action === 'login'
      ? h.store.login({ username: 'admin', password: 'synthetic-password' })
      : h.store[action]()
    response.resolve({ authenticated: action !== 'login' })
    await pending
    await transition
    assert.equal(h.store.isAuthenticated, action === 'login')
    assert.equal(h.store.sessionChecked, true)
  }
})

test('a stale completion cannot clear a newer in-flight check', async () => {
  const h = authHarness()
  const oldResponse = deferred()
  h.status(() => oldResponse.promise)
  const old = h.store.checkAuth()
  h.store.invalidateSession()
  const newResponse = deferred()
  h.status(() => newResponse.promise)
  const current = h.store.checkAuth()
  oldResponse.resolve({ authenticated: true })
  await old
  const shared = h.store.checkAuth()
  assert.equal(h.requests(), 2)
  newResponse.resolve({ authenticated: false })
  assert.deepEqual(await Promise.all([current, shared]), ['unauthenticated', 'unauthenticated'])
})

test('route guard blocks protected pages on uncertainty without redirecting to login', async () => {
  const h = authHarness()
  h.status(async () => {
    throw new ApiError('offline', 0)
  })
  const guard = routeGuard(h.store)
  const target = { path: '/accounts', fullPath: '/accounts?page=2' }
  const result = await guard(target)
  assert.deepEqual(JSON.parse(JSON.stringify(result)), {
    name: 'session-recovery',
    query: { redirect: target.fullPath },
  })
  assert.equal(await guard({ name: 'session-recovery', path: '/session-recovery' }), undefined)
  assert.equal(h.requests(), 1)
  h.status(async () => ({ authenticated: true }))
  assert.equal(await guard(target), undefined)
})

test('route guard still redirects genuinely unauthenticated sessions', async () => {
  const h = authHarness()
  h.status(async () => ({ authenticated: false }))
  assert.equal(await routeGuard(h.store)({ path: '/accounts', fullPath: '/accounts' }), '/login')
})

function recoveryHarness(checkAuth = async () => 'unavailable') {
  const mounted = []
  const unmounted = []
  const events = {}
  const timers = new Map()
  const navigations = []
  let nextTimer = 0
  let requests = 0
  const route = { query: { redirect: '/accounts?page=2' } }
  const document = { visibilityState: 'visible' }
  const { descriptor } = parse(readFileSync(source('views/session-recovery/index.vue'), 'utf8'))
  const script = compileScript(descriptor, { id: 'session-recovery' })
  const component = loadText(script.content, {
    '@lucide/vue': { RefreshCw: {} },
    '@vueuse/core': { useEventListener: (_target, name, handler) => { events[name] = handler } },
    'vue': { ...vue, onMounted: fn => mounted.push(fn), onUnmounted: fn => unmounted.push(fn) },
    'vue-router': { useRoute: () => route, useRouter: () => ({ replace: async path => navigations.push(path) }) },
    '@/components/base/BaseButton.vue': { default: {} },
    '@/stores/modules/auth': {
      useAuthStore: () => ({
        checkAuth: () => {
          requests += 1
          return checkAuth()
        },
      }),
    },
  }, {
    window: {},
    document,
    setTimeout: (fn, delay) => {
      const id = ++nextTimer
      timers.set(id, { fn, delay })
      return id
    },
    clearTimeout: id => timers.delete(id),
  }).default
  const bindings = component.setup({}, { expose: () => {} })
  mounted.forEach(fn => fn())
  return {
    bindings,
    events,
    timers,
    navigations,
    route,
    document,
    requests: () => requests,
    unmount: () => unmounted.forEach(fn => fn()),
    runTimer: async () => {
      const [id, timer] = timers.entries().next().value
      timers.delete(id)
      timer.fn()
      await tick()
    },
  }
}

test('automatic retries are bounded and manual recovery restores the original page', async () => {
  let available = false
  const h = recoveryHarness(async () => available ? 'authenticated' : 'unavailable')
  assert.equal(h.timers.size, 1)
  await h.runTimer()
  await h.runTimer()
  assert.equal(h.requests(), 2)
  assert.equal(h.timers.size, 0)
  assert.deepEqual(h.navigations, [])
  available = true
  await h.bindings.retry()
  assert.deepEqual(h.navigations, ['/accounts?page=2'])
})

test('network and visibility recovery recheck without duplicate in-flight requests', async () => {
  const response = deferred()
  const h = recoveryHarness(() => response.promise)
  h.document.visibilityState = 'hidden'
  h.events.visibilitychange()
  assert.equal(h.requests(), 0)
  h.document.visibilityState = 'visible'
  h.events.visibilitychange()
  h.events.online()
  assert.equal(h.requests(), 1)
  response.resolve('authenticated')
  await tick()
  assert.deepEqual(h.navigations, ['/accounts?page=2'])
})

test('recovery sends expired sessions to login and cannot navigate after unmount', async () => {
  const expired = recoveryHarness(async () => 'unauthenticated')
  await expired.bindings.retry()
  assert.deepEqual(expired.navigations, ['/login'])
  const response = deferred()
  const h = recoveryHarness(() => response.promise)
  const pending = h.bindings.retry()
  h.unmount()
  response.resolve('authenticated')
  await pending
  assert.equal(h.timers.size, 0)
  assert.deepEqual(h.navigations, [])
})

test('recovery rejects external destinations and recursive recovery paths', () => {
  const h = recoveryHarness()
  for (const value of [undefined, ['//example.invalid'], 'https://example.invalid', '//example.invalid', '/\\example.invalid', '/accounts\n', '/session-recovery?redirect=/accounts', '/SESSION-RECOVERY/', '/login', '/login/']) {
    h.route.query.redirect = value
    assert.equal(h.bindings.returnPath(), '/')
  }
  h.route.query.redirect = '/usage?model=example#records'
  assert.equal(h.bindings.returnPath(), '/usage?model=example#records')
  h.unmount()
})

test('the recovery screen is bundled eagerly so outages cannot block its lazy chunk', () => {
  const routes = readFileSync(source('router/routes.ts'), 'utf8')
  assert.match(routes, /import SessionRecoveryView from '@\/views\/session-recovery\/index\.vue'/)
  assert.match(routes, /name: 'session-recovery',\s*component: SessionRecoveryView/)
})
