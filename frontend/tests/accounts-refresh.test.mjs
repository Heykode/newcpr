/* eslint-disable test/no-import-node-test -- this regression uses Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

const require = createRequire(import.meta.url)

// Exercise the real composable and installed VueUse timer without a DOM or
// another test framework. Only the network and component lifecycle are isolated.
function loadModule(filename, dependencies, globals = {}) {
  const exports = {}
  const { outputText } = ts.transpileModule(readFileSync(filename, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, {
    exports,
    require: name => dependencies[name] ?? require(name),
    ...globals,
  }, { filename: String(filename) })
  return exports
}

function flushRequests() {
  return new Promise(resolve => setImmediate(resolve))
}

function accountResponse({ version = 1, page = 1, pageSize = 20, total = 1 } = {}) {
  return {
    items: [{
      id: 'acct_test',
      status: 'normal',
      healthTimeline: [{ requestCount: version }],
      scheduling: { inFlight: version },
    }],
    page: { page, pageSize, total, totalPages: Math.ceil(total / pageSize) },
    summary: { total, normal: total, quotaExhausted: 0, rateLimited: 0, disabled: 0, error: 0 },
  }
}

function createHarness(t) {
  const timers = new Map()
  const timeouts = new Map()
  const mounted = []
  let nextTimer = 1
  const browser = {
    window: {},
    document: {},
    setInterval: (callback, delay) => {
      const id = nextTimer++
      timers.set(id, { callback, delay })
      return id
    },
    clearInterval: id => timers.delete(id),
    setTimeout: (callback, delay) => {
      const id = nextTimer++
      timeouts.set(id, { callback, delay })
      return id
    },
    clearTimeout: id => timeouts.delete(id),
  }
  const shared = createRequire(require.resolve('@vueuse/core')).resolve('@vueuse/shared')
  const vueUse = loadModule(shared, { vue }, browser)
  const asyncUtils = loadModule(new URL('../src/utils/async.ts', import.meta.url), {})
  const requestState = loadModule(new URL('../src/composables/useRequestState.ts', import.meta.url), {
    vue,
    '@/api/request': { ApiError: class extends Error {} },
    '@/utils/async': asyncUtils,
  }, { AbortController })
  const pagedQuery = loadModule(new URL('../src/composables/usePagedQuery.ts', import.meta.url), {
    vue,
    './useRequestState': requestState,
  })
  const requests = []
  const errors = []
  const accounts = loadModule(new URL('../src/views/accounts/composables/useAccountsQuery.ts', import.meta.url), {
    'vue': { ...vue, onMounted: callback => mounted.push(callback) },
    '@vueuse/core': vueUse,
    '@/api': {
      getAccounts: (params, options) => new Promise((resolve, reject) => requests.push({ params, options, resolve, reject })),
    },
    '@/components/base/BaseToast': { toast: { error: message => errors.push(message) } },
    '@/composables/usePagedQuery': pagedQuery,
    '@/utils/async': asyncUtils,
  })
  const scope = vue.effectScope()
  t.after(() => {
    scope.stop()
    assert.equal(timers.size, 0, 'leaving the account page must dispose the timer')
  })
  const query = scope.run(() => accounts.useAccountsQuery())
  assert.equal(mounted.length, 1)
  assert.equal(vue.isRef(query.refreshing), true)
  assert.equal(vue.isReadonly(query.refreshing), true, 'refreshing is derived from pending requests')
  assert.equal(query.refreshing.value, false)
  assert.equal(query.loading.value, false)
  assert.equal(requests.length, 0, 'the timer must not duplicate the initial load')

  function timer() {
    assert.equal(timers.size, 1, 'the automatic refresh timer must actually start')
    const active = [...timers.values()][0]
    assert.equal(active.delay, 30_000)
    return active
  }
  const initialTimer = timer()

  return {
    query,
    scope,
    requests,
    errors,
    timers,
    mount: () => mounted[0](),
    tick: () => {
      assert.equal(timer(), initialTimer, 'refreshing must not restart or add a timer')
      return initialTimer.callback()
    },
    flushSearch: async () => {
      await vue.nextTick()
      assert.equal(timeouts.size, 1)
      const [id, timeout] = [...timeouts.entries()][0]
      assert.equal(timeout.delay, 250)
      timeouts.delete(id)
      timeout.callback()
      await flushRequests()
    },
    settle: async (result = accountResponse()) => {
      requests.at(-1).resolve(result)
      await flushRequests()
    },
  }
}

async function mountLoaded(harness, result = accountResponse()) {
  harness.mount()
  assert.equal(harness.requests.length, 1)
  assert.equal(harness.query.refreshing.value, true)
  assert.equal(harness.query.loading.value, true)
  await harness.settle(result)
  assert.equal(harness.query.refreshing.value, false)
  assert.equal(harness.query.loading.value, false)
}

function assertResult(query, result) {
  assert.equal(query.accounts.value, result.items, 'rows must come from the accepted response')
  assert.equal(query.accountSummary.value, result.summary, 'summary must come from the same response')
  assert.equal(query.totalAccounts.value, result.page.total)
  assert.deepEqual({ ...query.accountPagination.value }, {
    currentPage: result.page.page,
    pageSize: result.page.pageSize,
    total: result.page.total,
  })
}

test('initial and ordinary list loads block manual and automatic refresh until settled', async (t) => {
  const h = createHarness(t)
  const { query, requests } = h
  h.mount()
  assert.equal(requests.length, 1)
  assert.equal(requests[0].params.sortBy, 'addedAt')
  assert.equal(requests[0].params.sortDirection, 'desc', 'the default lists new accounts first')
  assert.equal(query.refreshing.value, true)
  assert.equal(query.loading.value, true)

  for (let tick = 0; tick < 3; tick += 1) {
    await h.tick()
    assert.equal(await query.refreshAccounts(), false)
    assert.equal(requests.length, 1, 'a slow initial load must remain the only request')
    assert.equal(query.refreshing.value, true)
    assert.equal(query.loading.value, true)
  }
  const initial = accountResponse()
  await h.settle(initial)
  assertResult(query, initial)
  assert.equal(query.refreshing.value, false)
  assert.equal(query.loading.value, false)

  const load = query.loadAccounts()
  assert.equal(requests.length, 2)
  assert.equal(query.refreshing.value, true)
  assert.equal(query.loading.value, true)
  await h.tick()
  assert.equal(await query.refreshAccounts(), false)
  assert.equal(requests.length, 2, 'ordinary list loads also block refresh')
  const updated = accountResponse({ version: 2, total: 2 })
  await h.settle(updated)
  assert.equal(await load, true)
  assertResult(query, updated)
  assert.equal(query.refreshing.value, false)
  assert.equal(query.loading.value, false)
  assert.deepEqual(h.errors, [])
})

test('active State collection speeds up the existing list timer and stops on readiness or opt-out', async (t) => {
  const h = createHarness(t)
  const result = accountResponse()
  Object.assign(result.items[0], {
    enabled: true,
    turnStateInjectionEnabled: true,
    turnState: { enabled: true, models: [{ refreshStatus: 'queued' }] },
  })
  await mountLoaded(h, result)
  const delay = () => {
    assert.equal(h.timers.size, 1)
    return [...h.timers.values()][0].delay
  }
  assert.equal(delay(), 3_000)
  const activeTimer = [...h.timers.values()][0]
  const refresh = activeTimer.callback()
  assert.equal(h.requests.length, 2)
  await activeTimer.callback()
  assert.equal(h.requests.length, 2, 'fast refresh must not overlap')
  const updated = structuredClone(result)
  updated.items[0].turnState.models[0].refreshStatus = 'ready'
  await h.settle(updated)
  await refresh
  assert.equal(delay(), 30_000)
  for (const changes of [
    { enabled: false },
    { status: 'error' },
    { turnStateInjectionEnabled: false },
    { turnState: { ...result.items[0].turnState, enabled: false } },
  ]) {
    const request = h.query.refreshAccounts()
    await h.settle({ ...result, items: [{ ...result.items[0], ...changes }] })
    await request
    assert.equal(delay(), 30_000)
  }
})

test('manual and automatic refresh share one timer and never overlap or queue repeated clicks', async (t) => {
  const h = createHarness(t)
  const { query, requests } = h
  const initial = accountResponse()
  await mountLoaded(h, initial)

  const manual = query.refreshAccounts()
  assert.equal(typeof manual.then, 'function', 'manual refresh returns an awaitable result')
  assert.equal(requests.length, 2)
  assert.equal(query.refreshing.value, true)
  assert.equal(query.loading.value, true, 'manual refresh uses the normal loading state')
  assertResult(query, initial)

  const repeated = [query.refreshAccounts(), query.refreshAccounts(), query.refreshAccounts()]
  assert.deepEqual(await Promise.all(repeated), [false, false, false])
  for (let tick = 0; tick < 3; tick += 1) {
    await h.tick()
    assert.equal(requests.length, 2)
  }
  const manualResult = accountResponse({ version: 2, total: 2 })
  await h.settle(manualResult)
  assert.equal(await manual, true)
  assertResult(query, manualResult)
  assert.equal(query.refreshing.value, false)
  assert.equal(query.loading.value, false)
  assert.equal(requests.length, 2, 'skipped clicks must not queue a later request')

  const automatic = h.tick()
  assert.equal(requests.length, 3)
  assert.equal(query.refreshing.value, true)
  assert.equal(query.loading.value, false, 'background refresh must not blank the table')
  assertResult(query, manualResult)
  for (let tick = 0; tick < 3; tick += 1) {
    await h.tick()
    assert.equal(await query.refreshAccounts(), false)
    assert.equal(requests.length, 3, 'slow background refresh blocks timer ticks and clicks')
    assert.equal(query.refreshing.value, true)
    assert.equal(query.loading.value, false)
  }
  const automaticResult = accountResponse({ version: 3, total: 3 })
  await h.settle(automaticResult)
  await automatic
  assertResult(query, automaticResult)
  assert.equal(query.refreshing.value, false)
  assert.equal(query.loading.value, false)
  assert.equal(requests.length, 3)

  const nextManual = query.refreshAccounts()
  assert.equal(requests.length, 4, 'a click after background completion starts a new request')
  assert.equal(query.loading.value, true)
  await h.settle()
  assert.equal(await nextManual, true)
  assert.equal(query.refreshing.value, false)
  assert.equal(query.loading.value, false)
  assert.deepEqual(h.errors, [])
})

test('both refresh paths preserve current paging, all filters and sort, updating rows and summary together', async (t) => {
  const h = createHarness(t)
  const { query, requests } = h
  await mountLoaded(h)

  query.searchQuery.value = 'account + & / ? #'
  await h.flushSearch()
  await h.settle()
  for (const [filter, value] of [
    [query.providerQuery, 'openai'],
    [query.statusQuery, 'normal'],
    [query.groupQuery, 'group-test'],
    [query.planTypeQuery, 'team'],
  ]) {
    filter.value = value
    await vue.nextTick()
    await h.settle()
  }
  query.handlePageSizeChange(50)
  await h.settle(accountResponse({ pageSize: 50, total: 125 }))
  const sort = { key: 'name', direction: 'asc' }
  query.handleSortChange(sort)
  await h.settle(accountResponse({ pageSize: 50, total: 125 }))
  query.handlePageChange(3)
  let accepted = accountResponse({ page: 3, pageSize: 50, total: 125 })
  await h.settle(accepted)

  const expectedParams = {
    page: 3,
    pageSize: 50,
    search: 'account + & / ? #',
    provider: 'openai',
    status: 'normal',
    groupId: 'group-test',
    planType: 'team',
    sortBy: 'name',
    sortDirection: 'asc',
  }
  const observed = []
  h.scope.run(() => vue.watch(
    () => [query.accounts.value, query.accountSummary.value, query.totalAccounts.value],
    value => observed.push(value),
  ))

  for (const mode of ['manual', 'automatic']) {
    const before = requests.length
    const refresh = mode === 'manual' ? query.refreshAccounts() : h.tick()
    assert.equal(requests.length, before + 1)
    assert.deepEqual({ ...requests.at(-1).params }, expectedParams)
    assert.equal(query.refreshing.value, true)
    assert.equal(query.loading.value, mode === 'manual')
    assertResult(query, accepted)
    assert.equal(query.sort.value, sort)
    assert.equal(query.searchQuery.value, expectedParams.search)
    assert.equal(query.providerQuery.value, expectedParams.provider)
    assert.equal(query.statusQuery.value, expectedParams.status)
    assert.equal(query.groupQuery.value, expectedParams.groupId)
    assert.equal(query.planTypeQuery.value, expectedParams.planType)
    const result = accountResponse({
      version: mode === 'manual' ? 2 : 3,
      page: 3,
      pageSize: 50,
      total: mode === 'manual' ? 126 : 127,
    })
    observed.length = 0
    await h.settle(result)
    const success = await refresh
    if (mode === 'manual')
      assert.equal(success, true)
    assertResult(query, result)
    assert.equal(query.refreshing.value, false)
    assert.equal(query.loading.value, false)
    assert.equal(observed.length, 1, 'render observers receive one coherent refresh')
    assert.equal(observed[0][0], result.items)
    assert.equal(observed[0][1], result.summary)
    assert.equal(observed[0][2], result.page.total)
    accepted = result
  }
  assert.deepEqual(h.errors, [])
})

for (const reload of ['diagnostic', 'silent load']) {
  test(`${reload} participates in pending tracking without showing loading or error toasts`, async (t) => {
    const h = createHarness(t)
    const { query, requests } = h
    const initial = accountResponse()
    await mountLoaded(h, initial)

    const refresh = reload === 'diagnostic'
      ? query.refreshAccountsSilently()
      : query.loadAccounts({ silent: true })
    assert.equal(requests.length, 2)
    assert.equal(query.refreshing.value, true)
    assert.equal(query.loading.value, false)
    await h.tick()
    assert.equal(await query.refreshAccounts(), false)
    assert.equal(requests.length, 2)
    requests.at(-1).reject(new Error('silent reload failed'))
    assert.equal(await refresh, false)
    assert.equal(query.refreshing.value, false)
    assert.equal(query.loading.value, false)
    assertResult(query, initial)
    assert.deepEqual(h.errors, [])

    const retry = query.refreshAccounts()
    assert.equal(requests.length, 3)
    assert.equal(query.refreshing.value, true)
    assert.equal(query.loading.value, true)
    const recovered = accountResponse({ version: 2, total: 2 })
    await h.settle(recovered)
    assert.equal(await retry, true)
    assertResult(query, recovered)
    assert.equal(query.refreshing.value, false)
    assert.equal(query.loading.value, false)
  })
}

for (const firstToFinish of ['older', 'newer']) {
  test(`refreshing tracks every pending list request when the ${firstToFinish} diagnostic reload finishes first`, async (t) => {
    const h = createHarness(t)
    const { query, requests } = h
    const initial = accountResponse()
    await mountLoaded(h, initial)

    const older = query.refreshAccountsSilently()
    const newer = query.refreshAccountsSilently()
    assert.equal(requests.length, 3, 'diagnostic reloads retain their existing execute behavior')
    assert.equal(query.refreshing.value, true)
    assert.equal(query.loading.value, false)
    const latest = accountResponse({ version: 3, total: 3 })
    const finishOlder = async () => {
      requests[1].resolve(accountResponse({ version: 2, total: 2 }))
      assert.equal(await older, false, 'an older response must not overwrite the current query')
    }
    const finishNewer = async () => {
      requests[2].resolve(latest)
      assert.equal(await newer, true)
    }
    await (firstToFinish === 'older' ? finishOlder() : finishNewer())
    assert.equal(query.refreshing.value, true, 'one completed request must not clear another pending request')
    assert.equal(query.loading.value, false)
    assertResult(query, firstToFinish === 'older' ? initial : latest)
    await h.tick()
    assert.equal(await query.refreshAccounts(), false)
    assert.equal(requests.length, 3)

    await (firstToFinish === 'older' ? finishNewer() : finishOlder())
    assertResult(query, latest)
    assert.equal(query.refreshing.value, false)
    const manual = query.refreshAccounts()
    assert.equal(requests.length, 4)
    assert.equal(query.loading.value, true)
    await h.settle()
    assert.equal(await manual, true)
    assert.equal(query.refreshing.value, false)
    assert.equal(query.loading.value, false)
    assert.deepEqual(h.errors, [])
  })
}

test('manual refresh failure keeps data and delegates notifications to the shared API', async (t) => {
  const h = createHarness(t)
  const { query, requests } = h
  const initial = accountResponse()
  await mountLoaded(h, initial)

  const failed = query.refreshAccounts()
  assert.equal(requests[1].options.silent, undefined, 'manual errors remain visible to the shared API notifier')
  assert.ok(requests[1].options.signal instanceof AbortSignal)
  assert.equal(query.refreshing.value, true)
  assert.equal(query.loading.value, true)
  assert.equal(await query.refreshAccounts(), false)
  requests[1].reject(new Error('manual refresh failed'))
  assert.equal(await failed, false)
  assert.deepEqual(h.errors, [], 'the list must not duplicate the shared API error notification')
  assertResult(query, initial)
  assert.equal(query.refreshing.value, false)
  assert.equal(query.loading.value, false)

  const retry = query.refreshAccounts()
  assert.equal(requests.length, 3)
  assert.equal(query.refreshing.value, true)
  assert.equal(query.loading.value, true)
  await h.tick()
  assert.equal(await query.refreshAccounts(), false)
  assert.equal(requests.length, 3)
  const recovered = accountResponse({ version: 2, total: 2 })
  await h.settle(recovered)
  assert.equal(await retry, true)
  assertResult(query, recovered)
  assert.equal(query.refreshing.value, false)
  assert.equal(query.loading.value, false)
  assert.deepEqual(h.errors, [], 'retry success must not report another error')
})

test('automatic refresh failure stays silent and the same timer recovers on its next tick', async (t) => {
  const h = createHarness(t)
  const { query, requests } = h
  const initial = accountResponse()
  await mountLoaded(h, initial)

  const failed = h.tick()
  assert.equal(requests[1].options.silent, true, 'automatic errors stay silent in the shared API notifier')
  assert.equal(query.refreshing.value, true)
  assert.equal(query.loading.value, false)
  await h.tick()
  assert.equal(await query.refreshAccounts(), false)
  assert.equal(requests.length, 2)
  requests[1].reject(new Error('automatic refresh failed'))
  await failed
  await flushRequests()
  assert.deepEqual(h.errors, [])
  assertResult(query, initial)
  assert.equal(query.refreshing.value, false)
  assert.equal(query.loading.value, false)

  const retry = h.tick()
  assert.equal(requests.length, 3)
  assert.equal(query.refreshing.value, true)
  assert.equal(query.loading.value, false)
  const recovered = accountResponse({ version: 2, total: 2 })
  await h.settle(recovered)
  await retry
  assertResult(query, recovered)
  assert.equal(query.refreshing.value, false)
  assert.equal(query.loading.value, false)
  assert.deepEqual(h.errors, [])
})

for (const outcome of ['success', 'error']) {
  test(`scope disposal stops the timer and ignores a pending manual refresh ${outcome}`, async (t) => {
    const h = createHarness(t)
    const { query, requests } = h
    const initial = accountResponse()
    await mountLoaded(h, initial)
    const manual = query.refreshAccounts()
    assert.equal(requests.length, 2)
    assert.equal(query.refreshing.value, true)
    assert.equal(query.loading.value, true)

    h.scope.stop()
    assert.equal(h.timers.size, 0)
    assert.equal(query.loading.value, false)
    assertResult(query, initial)
    if (outcome === 'success')
      requests[1].resolve(accountResponse({ version: 9, total: 90, page: 2, pageSize: 50 }))
    else
      requests[1].reject(new Error('request failed after leaving the page'))
    assert.equal(await manual, false)
    await flushRequests()
    assertResult(query, initial)
    assert.equal(query.refreshing.value, false)
    assert.equal(query.loading.value, false)
    assert.equal(h.timers.size, 0, 'settling the request must not restart the timer')
    assert.equal(requests.length, 2)
    assert.deepEqual(h.errors, [], 'disposed requests must not emit an error toast')
  })
}

test('sorting can ascend, descend or clear while refresh preserves the selected order', async (t) => {
  const h = createHarness(t)
  const { query, requests } = h
  await mountLoaded(h)

  for (const sort of [{ key: 'addedAt', direction: 'asc' }, { key: 'addedAt', direction: 'desc' }, undefined]) {
    query.page.value = 3
    query.handleSortChange(sort)
    assert.equal(query.page.value, 1)
    assert.equal(query.sort.value, sort, 'the table must be able to leave descending order')
    assert.equal(requests.at(-1).params.sortDirection, sort?.direction ?? 'desc')
    assert.equal(query.refreshing.value, true)
    await h.settle()

    const manual = query.refreshAccounts()
    assert.equal(requests.at(-1).params.sortBy, 'addedAt')
    assert.equal(requests.at(-1).params.sortDirection, sort?.direction ?? 'desc')
    assert.equal(query.sort.value, sort, 'refresh must not force a cleared UI sort back to descending')
    await h.settle()
    assert.equal(await manual, true)
    assert.equal(query.refreshing.value, false)
    assert.equal(query.loading.value, false)
  }
  assert.deepEqual(h.errors, [])
})

test('account mutations reread authoritative filtered rows and summary and invalidate an older refresh', async (t) => {
  const h = createHarness(t)
  const { query, requests } = h
  await mountLoaded(h)
  query.statusQuery.value = 'normal'
  await vue.nextTick()
  const initial = accountResponse({ total: 2 })
  await h.settle(initial)

  const older = query.refreshAccountsSilently()
  const olderRequest = requests.at(-1)
  const replacement = query.replaceAccount({ ...initial.items[0], status: 'disabled' })
  const reread = requests.at(-1)
  assert.equal(olderRequest.options.signal.aborted, true)
  assert.notEqual(reread, olderRequest)
  assert.equal(reread.params.status, 'normal')
  assert.equal(reread.options.silent, undefined, 'mutation rereads use the shared visible failure path')
  assert.ok(reread.options.signal instanceof AbortSignal)
  assertResult(query, initial)

  const accepted = accountResponse({ version: 3 })
  accepted.items[0].id = 'acct_remaining'
  reread.resolve(accepted)
  assert.equal(await replacement, false, 'the caller can drop selection only after an accepted filtered reread')
  assertResult(query, accepted)
  assert.equal(query.refreshing.value, true, 'the invalidated request still owns pending work until settlement')

  olderRequest.resolve(accountResponse({ version: 2, total: 2 }))
  assert.equal(await older, false)
  assertResult(query, accepted)
  assert.equal(query.refreshing.value, false)
  assert.equal(query.loading.value, false)
  assert.deepEqual(h.errors, [])
})

test('account mutation reread falls back from an emptied last page without publishing mismatched summary data', async (t) => {
  const h = createHarness(t)
  const { query, requests } = h
  const initial = accountResponse({ page: 2, total: 21 })
  await mountLoaded(h, initial)
  const replacement = query.replaceAccount({ ...initial.items[0], status: 'disabled' })
  assert.equal(requests.at(-1).params.page, 2)
  const emptyLastPage = accountResponse({ page: 2, total: 20 })
  emptyLastPage.items = []
  const failedPage = requests.at(-1)
  failedPage.resolve(emptyLastPage)
  await flushRequests()

  assert.equal(requests.length, 3)
  assert.equal(requests.at(-1).params.page, 1)
  assert.equal(failedPage.options.signal.aborted, true)
  assert.equal(query.refreshing.value, true)
  assert.equal(query.loading.value, true)
  assert.equal(query.accounts.value, initial.items)
  assert.equal(query.accountSummary.value, initial.summary)
  await h.tick()
  assert.equal(await query.refreshAccounts(), false)
  assert.equal(requests.length, 3, 'manual and timed refresh stay blocked during page fallback')

  const accepted = accountResponse({ version: 2, total: 20 })
  accepted.items[0].id = 'acct_remaining'
  await h.settle(accepted)
  assert.equal(await replacement, false)
  assertResult(query, accepted)
  assert.equal(query.refreshing.value, false)
  assert.equal(query.loading.value, false)
  assert.deepEqual(h.errors, [])
})

test('failed or superseded mutation rereads preserve selection instead of inferring removal from stale rows', async (t) => {
  const h = createHarness(t)
  const { query, requests } = h
  const initial = accountResponse()
  await mountLoaded(h, initial)
  const updated = { ...initial.items[0], status: 'disabled' }
  const failed = query.replaceAccount(updated)
  requests.at(-1).reject(new Error('authoritative reread unavailable'))
  assert.equal(await failed, true)
  assertResult(query, initial)
  assert.equal(query.refreshing.value, false)
  assert.deepEqual(h.errors, [], 'the composable must not duplicate shared request notifications')

  const superseded = query.replaceAccount(updated)
  const stale = requests.at(-1)
  const newer = query.loadAccounts()
  const accepted = accountResponse({ version: 3 })
  await h.settle(accepted)
  assert.equal(await newer, true)
  stale.resolve({ ...accountResponse({ total: 0 }), items: [] })
  assert.equal(await superseded, true)
  assertResult(query, accepted)
  assert.equal(query.refreshing.value, false)
  assert.equal(query.loading.value, false)
})

function toggleHarness(harness, row) {
  const writes = []
  const selectedIds = vue.ref(new Set([row.id, 'acct_other_page']))
  const asyncUtils = loadModule(new URL('../src/utils/async.ts', import.meta.url), {})
  const notifications = { toast: { success() {}, error() {} } }
  const actions = loadModule(new URL('../src/composables/useAsyncAction.ts', import.meta.url), {
    'vue': vue,
    '@/api/request': { ApiError: class extends Error {} },
    '@/utils/async': asyncUtils,
    '@/components/base/BaseToast': notifications,
  })
  const ids = loadModule(new URL('../src/composables/useIdSet.ts', import.meta.url), { vue })
  const mutations = loadModule(new URL('../src/views/accounts/composables/useAccountMutations.ts', import.meta.url), {
    'vue': vue,
    '@/api': {
      updateAccount: body => new Promise((resolve, reject) => writes.push({ body, resolve, reject })),
      batchUpdateAccounts: body => new Promise((resolve, reject) => writes.push({ body, resolve, reject })),
    },
    '@/components/base/BaseToast': notifications,
    '@/composables/useAsyncAction': actions,
    '@/composables/useIdSet': ids,
    '@/composables/useDownload': { useDownload: () => ({ downloadJson() {} }) },
    '@/utils/async': asyncUtils,
    './useAccountOnboarding': { useAccountOnboarding: () => ({}) },
  })
  const state = harness.scope.run(() => mutations.useAccountMutations({
    accounts: harness.query.accounts,
    selectedIds,
    reload: harness.query.loadAccounts,
    replaceAccount: harness.query.replaceAccount,
  }))
  return { state, writes, selectedIds }
}

for (const enabled of [false, true]) {
  test(`turn state switch ${enabled ? 'on' : 'off'} only patches its own field and preserves concurrent scheduling changes`, async (t) => {
    const h = createHarness(t)
    const initial = accountResponse()
    const row = {
      ...initial.items[0],
      provider: 'openai',
      enabled: true,
      turnStateInjectionEnabled: !enabled,
      concurrencyLimit: 2,
      weight: 100,
      groups: [{ id: 'grp_test' }],
    }
    initial.items = [row]
    await mountLoaded(h, initial)
    const { state, writes, selectedIds } = toggleHarness(h, row)
    const operation = state.handleToggleTurnState(row, enabled)
    await state.handleToggleTurnState(row, enabled)
    assert.equal(writes.length, 1, 'duplicate state switches must share the in-flight guard')
    assert.deepEqual(structuredClone(writes[0].body), {
      accountIds: [row.id],
      turnStateInjectionEnabled: enabled,
    })
    writes[0].resolve({})
    await flushRequests()
    const updated = {
      ...initial,
      items: [{
        ...row,
        enabled: false,
        weight: 250,
        concurrencyLimit: 7,
        groups: [{ id: 'grp_changed' }],
        turnStateInjectionEnabled: enabled,
      }],
    }
    await h.settle(updated)
    await operation
    assertResult(h.query, updated)
    assert.equal(selectedIds.value.size, 2)
    await state.handleToggleTurnState({ ...row, provider: 'xai' }, enabled)
    assert.equal(writes.length, 1, 'non-OpenAI accounts must not expose this mutation')
  })
}

for (const enabled of [false, true]) {
  test(`switch ${enabled ? 'on from paused' : 'off from normal'} rereads the filter immediately and removes only the filtered-out selection`, async (t) => {
    const h = createHarness(t)
    const initial = accountResponse()
    const row = {
      ...initial.items[0],
      enabled: !enabled,
      turnStateInjectionEnabled: true,
      status: enabled ? 'disabled' : 'normal',
      concurrencyLimit: 2,
      weight: 100,
      groups: [{ id: 'grp_test' }],
    }
    initial.items = [row]
    await mountLoaded(h, initial)
    h.query.statusQuery.value = row.status
    await vue.nextTick()
    await h.settle(initial)
    const { state, writes, selectedIds } = toggleHarness(h, row)
    const count = h.requests.length
    const operation = state.handleToggleEnabled(row, enabled)
    await state.handleToggleEnabled(row, enabled)
    assert.equal(writes.length, 1, 'duplicate switch clicks cannot send a second mutation')
    assert.equal(h.requests.length, count, 'reread must wait for confirmed switch write')
    assert.deepEqual({ ...writes[0].body, groupIds: [...writes[0].body.groupIds] }, {
      accountId: row.id,
      enabled,
      concurrencyLimit: 2,
      weight: 100,
      groupIds: ['grp_test'],
    })
    writes[0].resolve({})
    await flushRequests()
    assert.equal(h.requests.length, count + 1, 'no 30-second timer tick is needed')
    assert.equal(h.requests.at(-1).params.status, row.status)
    assert.ok(selectedIds.value.has(row.id), 'retain selection until authoritative reread')
    const result = { ...accountResponse({ total: 0 }), items: [] }
    result.summary = {
      total: 1,
      normal: 0,
      quotaExhausted: 0,
      rateLimited: 0,
      disabled: enabled ? 0 : 1,
      error: enabled ? 1 : 0,
    }
    await h.settle(result)
    await operation
    assertResult(h.query, result)
    assert.deepEqual([...selectedIds.value], ['acct_other_page'])
  })
}

test('switch failures and failed rereads preserve rows and selection; successful all-status reread shows paused', async (t) => {
  const h = createHarness(t)
  const initial = accountResponse()
  const row = { ...initial.items[0], enabled: true, groups: [], weight: 100, concurrencyLimit: null }
  initial.items = [row]
  await mountLoaded(h, initial)
  const { state, writes, selectedIds } = toggleHarness(h, row)
  const failedWrite = state.handleToggleEnabled(row, false)
  writes.at(-1).reject(new Error('write failed'))
  await failedWrite
  assert.equal(h.requests.length, 1)
  assertResult(h.query, initial)
  assert.equal(selectedIds.value.size, 2)

  const failedRead = state.handleToggleEnabled(row, false)
  writes.at(-1).resolve({})
  await flushRequests()
  h.requests.at(-1).reject(new Error('read failed'))
  await failedRead
  assertResult(h.query, initial)
  assert.equal(selectedIds.value.size, 2)

  const success = state.handleToggleEnabled(row, false)
  writes.at(-1).resolve({})
  await flushRequests()
  const paused = {
    ...initial,
    items: [{ ...row, enabled: false, status: 'disabled' }],
    summary: { ...initial.summary, normal: 0, disabled: 1 },
  }
  await h.settle(paused)
  await success
  assertResult(h.query, paused)
  assert.equal(selectedIds.value.size, 2, 'all-status view keeps a still-visible row selected')
})
