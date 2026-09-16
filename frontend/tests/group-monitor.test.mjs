/* eslint-disable test/no-import-node-test -- regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

const require = createRequire(import.meta.url)
const tick = () => new Promise(resolve => setImmediate(resolve))
function load(path, dependencies = {}, globals = {}) {
  const file = new URL(`../src/views/accounts/${path}.ts`, import.meta.url)
  const exports = {}
  const { outputText } = ts.transpileModule(readFileSync(file, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, {
    exports,
    require: name => dependencies[name] ?? require(name),
    AbortController,
    ...globals,
  }, { filename: file.pathname })
  return exports
}
const presentation = load('components/group-monitor-presentation')
const group = id => ({ id: `grp_${id}`, createdAt: `2026-01-0${id}`, enabled: true, memberCount: 1 })

function harness(t, initialStorage = new Map(), initialGroups = [1, 2, 3, 4, 5].map(group)) {
  let time = Date.parse('2026-09-15T00:00:00Z')
  let interval
  let intervalMs
  let onStorage
  const pending = []
  const visibility = vue.ref('visible')
  const groups = vue.ref(initialGroups)
  const size = vue.ref(2)
  const module = load('composables/useGroupMonitor', {
    vue,
    '../components/group-monitor-presentation': presentation,
    '@vueuse/core': {
      useDocumentVisibility: () => visibility,
      useIntervalFn: (callback, ms) => {
        interval = callback
        intervalMs = ms
      },
      useEventListener: (_target, _name, callback) => onStorage = callback,
    },
    '@/api': {
      getGroupMonitor: (ids, options) => new Promise((resolve, reject) => {
        pending.push({ ids, ...options, resolve, reject })
      }),
    },
  }, {
    window: {},
    Date: class extends Date { static now() { return time } },
    localStorage: {
      getItem: key => initialStorage.get(key),
      setItem: (key, value) => initialStorage.set(key, value),
    },
  })
  const scope = vue.effectScope()
  const state = scope.run(() => module.useGroupMonitor(groups, size))
  t.after(() => scope.stop())
  return {
    state,
    pending,
    visibility,
    groups,
    size,
    scope,
    storage: initialStorage,
    intervalMs,
    emitStorage: key => onStorage({ key }),
    advance(ms) {
      time += ms
      interval()
    },
    reply(index, viewerScope = 'admin:one') {
      const request = pending[index]
      request.resolve({
        viewerScope,
        generatedAt: new Date(time).toISOString(),
        items: request.ids.map(id => ({ id, remainingUsd: index + 1 })),
      })
    },
  }
}

test('pin parsing rejects malformed data and sorting leaves source order untouched', () => {
  assert.equal(presentation.readPinnedGroups({}).length, 0)
  assert.deepEqual([...presentation.readPinnedGroups(['grp_1', 'grp_1', null, '../x', 'grp_2'])], ['grp_1', 'grp_2'])
  const groups = [3, 2, 1].map(group)
  assert.deepEqual([...presentation.orderMonitorGroups(groups, ['grp_2'])].map(g => g.id), ['grp_2', 'grp_1', 'grp_3'])
  assert.deepEqual(groups.map(g => g.id), ['grp_3', 'grp_2', 'grp_1'])
  assert.equal(presentation.monitorMoney(Infinity), '未知')
  assert.equal(presentation.monitorMoney(null, 'learning'), '暂无估值')
  assert.equal(presentation.monitorMoney(0), '$0.00')
  assert.equal(presentation.monitorEta(null, 'idle'), '暂无消耗')
  assert.equal(presentation.monitorEta(Infinity, 'ready'), '未知')
  assert.equal(presentation.monitorEta(50, 'ready'), '50 分钟')
})

test('page requests abort older work; late replies cannot replace the visible page', async (t) => {
  const h = harness(t)
  assert.deepEqual([...h.pending[0].ids], ['grp_1', 'grp_2'])
  assert.equal(h.pending[0].refreshForecasts, false)
  h.state.page.value = 1
  await tick()
  assert.equal(h.pending[0].signal.aborted, true)
  assert.equal(h.pending[1].refreshForecasts, false)
  h.reply(1)
  await tick()
  h.reply(0)
  await tick()
  assert.equal(h.state.records.value.has('grp_1'), false)
  assert.equal(h.state.records.value.has('grp_3'), true)
  assert.equal(h.state.stale.value, false)
  h.state.page.value = 0
  await tick()
  assert.equal(h.state.stale.value, true)
  h.pending[2].reject(new Error('offline'))
  await tick()
  assert.equal(h.state.error.value, true)
})

test('pin scope, latest-first order, pagination and cross-tab restoration are independent of account data', async (t) => {
  const h = harness(t, new Map([['cpr.accounts.monitor.pins.admin:one', '["grp_5"]']]))
  h.reply(0)
  await tick()
  assert.deepEqual([...h.state.visible.value].map(g => g.id), ['grp_5', 'grp_1'])
  h.reply(1)
  await tick()
  h.state.togglePin('grp_3')
  await tick()
  assert.deepEqual([...h.state.pins.value], ['grp_3', 'grp_5'])
  assert.equal(h.state.page.value, 0)
  assert.equal(h.storage.get('cpr.accounts.monitor.pins.admin:one'), '["grp_3","grp_5"]')
  h.reply(2, 'admin:two')
  await tick()
  assert.equal(h.state.pins.value.length, 0)
  assert.equal(h.storage.get('cpr.accounts.monitor.pins.admin:two'), undefined)
  h.state.page.value = 1
  h.storage.set('cpr.accounts.monitor.pins.admin:two', '["grp_4"]')
  h.emitStorage('cpr.accounts.monitor.pins.admin:two')
  await tick()
  assert.equal(h.state.page.value, 0)
  assert.equal(h.state.visible.value[0].id, 'grp_4')
  assert.deepEqual(h.groups.value.map(g => g.id), [1, 2, 3, 4, 5].map(id => `grp_${id}`))
})

test('hidden views stop polling; stale cached pages, resize and disposal remain safe', async (t) => {
  const h = harness(t)
  h.reply(0)
  await tick()
  h.state.page.value = 1
  await tick()
  h.reply(1)
  await tick()
  h.advance(60_000)
  h.reply(2)
  await tick()
  h.state.page.value = 0
  await tick()
  assert.equal(h.state.stale.value, true)
  h.visibility.value = 'hidden'
  await tick()
  assert.equal(h.pending[3].signal.aborted, true)
  const count = h.pending.length
  h.advance(60_000)
  assert.equal(h.pending.length, count)
  h.state.page.value = 2
  h.groups.value = [group(1)]
  await tick()
  assert.equal(h.state.page.value, 0)
  h.visibility.value = 'visible'
  await tick()
  assert.equal(h.pending.length, count + 1)
  assert.equal(h.pending.at(-1).refreshForecasts, false)
  h.scope.stop()
  assert.equal(h.pending.at(-1).signal.aborted, true)
})

test('an incomplete reply is an error, never a healthy empty group', async (t) => {
  const h = harness(t)
  h.pending[0].resolve({ viewerScope: 'admin:one', generatedAt: new Date().toISOString(), items: [] })
  await tick()
  assert.equal(h.state.error.value, true)
  assert.equal(h.state.records.value.size, 0)
})

test('automatic reads share snapshots while manual refresh samples without duplicate requests', async (t) => {
  const h = harness(t)
  assert.equal(h.intervalMs, 10_000)
  assert.equal(h.pending[0].refreshForecasts, false)
  h.reply(0)
  await tick()
  h.state.page.value = 1
  await tick()
  h.reply(1)
  await tick()

  const manual = h.state.refreshNow()
  assert.equal(h.pending.length, 3)
  assert.equal(h.pending[2].refreshForecasts, true)
  assert.equal(h.pending[2].silent, true)
  assert.deepEqual([...h.pending[2].ids], ['grp_3', 'grp_4'])
  assert.equal(h.state.loading.value, true)
  h.state.refreshNow()
  h.advance(10_000)
  assert.equal(h.pending.length, 3)
  assert.equal(h.pending[2].signal.aborted, false)
  h.pending[2].reject(new Error('offline'))
  await manual
  assert.equal(h.state.loading.value, false)
  assert.equal(h.state.error.value, true)
  assert.equal(h.state.page.value, 1)
  assert.equal(h.state.records.value.get('grp_3').remainingUsd, 2)

  const retry = h.state.refreshNow()
  assert.equal(h.pending[3].refreshForecasts, true)
  h.reply(3)
  await retry
  assert.equal(h.state.error.value, false)
  assert.equal(h.state.page.value, 1)
  h.advance(10_000)
  assert.equal(h.pending.length, 5)
  assert.equal(h.pending[4].refreshForecasts, false)
  h.visibility.value = 'hidden'
  await tick()
  h.state.refreshNow()
  assert.equal(h.pending.length, 5)
})

test('every ten-second monitor tick reads the shared result without initiating sampling', async (t) => {
  const h = harness(t)
  h.reply(0)
  await tick()
  for (let round = 1; round <= 31; round++) {
    h.advance(10_000)
    assert.equal(h.pending.length, round + 1)
    assert.equal(h.pending[round].refreshForecasts, false)
    assert.deepEqual([...h.pending[round].ids], ['grp_1', 'grp_2'])
    h.reply(round)
    await tick()
    assert.equal(h.state.records.value.get('grp_1').remainingUsd, round + 1)
    assert.equal(h.state.loading.value, false)
    assert.equal(h.state.page.value, 0)
  }
})

test('successful reads of an old server snapshot remain stale until a new sample arrives', async (t) => {
  const h = harness(t)
  const generatedAt = '2026-09-15T00:00:00Z'
  h.reply(0)
  await tick()
  for (let round = 1; round <= 5; round++) {
    h.advance(10_000)
    h.pending[round].resolve({
      viewerScope: 'admin:one',
      generatedAt,
      items: h.pending[round].ids.map(id => ({ id, remainingUsd: 100 })),
    })
    await tick()
  }
  assert.equal(h.state.error.value, false)
  assert.equal(h.state.stale.value, true)
  h.advance(10_000)
  h.reply(6)
  await tick()
  assert.equal(h.state.stale.value, false)
})

test('monitor API passes explicit local forecast refresh and preserves request options', () => {
  const requests = []
  const api = load('../../api/modules/group-monitor', {
    '../request': { __esModule: true, default: config => requests.push(config) },
  })
  const signal = new AbortController().signal
  api.getGroupMonitor(['grp_1', 'grp_2'], { signal, silent: true })
  api.getGroupMonitor(['grp_3'], { signal, silent: true, refreshForecasts: true })
  api.getGroupMonitor(['grp_4'], { refreshForecasts: false })
  assert.deepEqual({ ...requests[0].params }, { groupIds: 'grp_1,grp_2' })
  assert.deepEqual({ ...requests[1].params }, { groupIds: 'grp_3', refreshForecasts: true })
  assert.deepEqual({ ...requests[2].params }, { groupIds: 'grp_4' })
  assert.equal(requests[1].signal, signal)
  assert.equal(requests[1].silent, true)
  assert.equal(requests[1].refreshForecasts, undefined)
  assert.ok(requests.every(config => config.method === 'GET' && config.url === '/api/admin/account-groups/monitor'))
})

test('only bound or pinned groups display, including disabled and unused bound groups', () => {
  const groups = [
    { ...group(1), memberCount: 0 },
    { ...group(2), enabled: false, accountSummary: { available: 0 }, usage: { todayUsd: '0' } },
    { ...group(3), memberCount: 0 },
  ]
  assert.deepEqual([...presentation.orderMonitorGroups(groups, ['grp_3'])].map(g => g.id), ['grp_3', 'grp_2'])
  assert.equal(groups.length, 3)
})

test('all-empty catalogs restore scoped pins without displaying an unpinned bootstrap group', async (t) => {
  const groups = [1, 2].map(id => ({ ...group(id), memberCount: 0 }))
  const h = harness(t, new Map([['cpr.accounts.monitor.pins.admin:one', '["grp_2"]']]), groups)
  assert.equal(h.state.visible.value.length, 0)
  assert.deepEqual([...h.pending[0].ids], ['grp_1'])
  h.reply(0)
  await tick()
  assert.deepEqual([...h.state.visible.value].map(g => g.id), ['grp_2'])
  h.reply(1)
  await tick()
  h.state.togglePin('grp_2')
  await tick()
  assert.equal(h.state.displayedCount.value, 0)
  h.advance(60_000)
  assert.equal(h.pending.length, 2)
  h.state.togglePin('grp_1')
  await tick()
  assert.equal(h.state.visible.value[0].id, 'grp_1')
})

test('three groups per page and membership changes keep pinned empty groups and clamp the page', async (t) => {
  const h = harness(t)
  h.reply(0)
  await tick()
  h.size.value = 3
  await tick()
  assert.deepEqual([...h.pending.at(-1).ids], ['grp_1', 'grp_2', 'grp_3'])
  assert.equal(h.state.totalPages.value, 2)
  h.state.page.value = 1
  await tick()
  assert.deepEqual([...h.state.visible.value].map(g => g.id), ['grp_4', 'grp_5'])
  h.state.togglePin('grp_4')
  h.groups.value = h.groups.value.map(g => ({ ...g, memberCount: 0 }))
  await tick()
  assert.equal(h.state.page.value, 0)
  assert.equal(h.state.displayedCount.value, 1)
  assert.equal(h.state.visible.value[0].id, 'grp_4')
  h.state.togglePin('grp_4')
  await tick()
  assert.equal(h.state.displayedCount.value, 0)
})
