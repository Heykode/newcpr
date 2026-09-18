/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import { useStorage } from '@vueuse/core'
import ts from 'typescript'
import * as vue from 'vue'

const require = createRequire(import.meta.url)
const prefix = 'codex-proxy:table-columns:'

function load(filename, dependencies = {}) {
  const exports = {}
  const { outputText } = ts.transpileModule(readFileSync(filename, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, { exports, require: name => dependencies[name] ?? require(name) })
  return exports
}

const { usageRecordColumns, opsErrorColumns } = load(new URL('../src/views/usage/constants.ts', import.meta.url), {
  './utils/format': { formatProvider: value => value },
})
const { resolveColumns } = load(new URL('../src/components/base/BaseTable/columns.ts', import.meta.url))

test('State preview uses a compact column without widening adjacent numeric columns', () => {
  const columns = resolveColumns(usageRecordColumns)
  assert.equal(columns.find(column => column.key === 'turnState').basisWidth, 144)
  assert.equal(columns.find(column => column.key === 'tokenDetails').basisWidth, 184)
  assert.equal(columns.find(column => column.key === 'billing').basisWidth, 144)
})

class MemoryStorage {
  values = new Map()
  writes = []

  getItem(key) {
    return this.values.get(key) ?? null
  }

  setItem(key, value) {
    this.values.set(key, value)
    this.writes.push([key, value])
  }

  removeItem(key) {
    this.values.delete(key)
  }
}

function mount(t, source = usageRecordColumns, tableId = 'usage-records', storage = new MemoryStorage(), window) {
  const errors = []
  const module = load(new URL('../src/components/base/BaseTable/useTableColumns.ts', import.meta.url), {
    'vue': vue,
    // Inject only the browser boundary; persistence and reactivity use real VueUse.
    '@vueuse/core': {
      useStorage: (key, defaults, _storage, options) =>
        useStorage(key, defaults, storage, { ...options, window, onError: error => errors.push(error) }),
    },
  })
  const scope = vue.effectScope()
  const state = scope.run(() => module.useTableColumns(source, tableId))
  t.after(() => scope.stop())
  return { ...state, storage, errors, stop: () => scope.stop() }
}

function keys(state) {
  return Array.from(state.visibleColumns.value, column => column.key)
}

function option(state, key) {
  return state.columnOptions.value.find(column => column.key === key)
}

test('initial usage tables retain every existing column and do not write defaults', (t) => {
  for (const columns of [usageRecordColumns, opsErrorColumns]) {
    const state = mount(t, columns)
    assert.deepEqual(keys(state), Array.from(columns, column => column.key))
    assert.deepEqual(state.storage.writes, [])
    assert.deepEqual(state.errors, [])
    assert.equal(state.visibleColumns.value[0], columns[0], 'preserve column identity and formatters')
  }
})

test('visibility persists across disposal and remount without reactive write loops', async (t) => {
  const state = mount(t)
  state.setColumnVisible('model', false)
  await vue.nextTick()
  assert.equal(option(state, 'model').visible, false)
  assert.equal(state.storage.getItem(`${prefix}usage-records`), '{"model":false}')
  const afterHide = state.storage.writes.length
  for (let i = 0; i < 5; i += 1) {
    state.setColumnVisible('model', false)
    await vue.nextTick()
    assert.equal(option(state, 'model').visible, false)
  }
  assert.equal(state.storage.writes.length, afterHide)
  state.stop()

  const reopened = mount(t, usageRecordColumns, 'usage-records', state.storage)
  assert.equal(option(reopened, 'model').visible, false)
  reopened.setColumnVisible('model', true)
  await vue.nextTick()
  assert.equal(state.storage.getItem(`${prefix}usage-records`), '{}', 'returning to default removes the override')
  assert.deepEqual(reopened.errors, [])
})

test('corrupt JSON and non-object storage are usable before the first render', (t) => {
  for (const raw of ['{broken', 'null', '[]', '["model"]', 'false', '0', '"old"']) {
    const storage = new MemoryStorage()
    storage.values.set(`${prefix}usage-records`, raw)
    const state = mount(t, usageRecordColumns, 'usage-records', storage)
    assert.deepEqual(keys(state), Array.from(usageRecordColumns, column => column.key), raw)
    assert.deepEqual(state.errors, [], raw)
  }
})

test('only boolean overrides apply and unknown keys cannot add or hide table fields', async (t) => {
  const storage = new MemoryStorage()
  storage.values.set(`${prefix}usage-records`, '{"model":false,"route":"false","provider":null,"clientIp":0,"billing":[],"removed":true,"__proto__":false}')
  const state = mount(t, usageRecordColumns, 'usage-records', storage)
  assert.equal(option(state, 'model').visible, false)
  for (const key of ['route', 'provider', 'clientIp', 'billing'])
    assert.equal(option(state, key).visible, true, key)
  assert.equal(option(state, 'removed'), undefined)
  assert.equal(option(state, '__proto__'), undefined)
  await vue.nextTick()
  const writesAfterNormalization = storage.writes.length
  assert.deepEqual(JSON.parse(storage.getItem(`${prefix}usage-records`)), JSON.parse('{"model":false,"removed":true,"__proto__":false}'))
  state.setColumnVisible('removed', false)
  state.setColumnVisible('missing', true)
  await vue.nextTick()
  assert.equal(storage.writes.length, writesAfterNormalization)
})

test('new columns follow their defaults while previously hidden columns remain hidden', async (t) => {
  const source = vue.shallowRef([
    { key: 'identity', label: 'Identity', hideable: false },
    { key: 'model', label: 'Model' },
  ])
  const state = mount(t, () => source.value)
  state.setColumnVisible('model', false)
  await vue.nextTick()
  source.value = [
    ...source.value,
    { key: 'newVisible', label: 'New visible' },
    { key: 'newHidden', label: 'New hidden', defaultHidden: true },
  ]
  assert.deepEqual(keys(state), ['identity', 'newVisible'])
  state.setColumnVisible('newHidden', true)
  await vue.nextTick()
  state.stop()

  const reopened = mount(t, source, 'usage-records', state.storage)
  assert.deepEqual(keys(reopened), ['identity', 'newVisible', 'newHidden'])
  reopened.resetColumns()
  await vue.nextTick()
  assert.deepEqual(keys(reopened), ['identity', 'model', 'newVisible'])
  assert.equal(state.storage.getItem(`${prefix}usage-records`), '{}')
  const resetReload = mount(t, source.value, 'usage-records', state.storage)
  assert.deepEqual(keys(resetReload), ['identity', 'model', 'newVisible'])
})

test('success and error preferences and reset remain isolated from each other and accounts', async (t) => {
  const storage = new MemoryStorage()
  storage.values.set('cpr.accounts.visible-columns', '{"version":2,"keys":[]}')
  const success = mount(t, usageRecordColumns, 'usage-records', storage)
  const errors = mount(t, opsErrorColumns, 'ops-errors', storage)
  success.setColumnVisible('model', false)
  errors.setColumnVisible('route', false)
  await vue.nextTick()
  assert.equal(option(success, 'route').visible, true)
  assert.equal(option(errors, 'model').visible, true)
  success.resetColumns()
  await vue.nextTick()
  assert.equal(option(success, 'model').visible, true)
  assert.equal(option(errors, 'route').visible, false)
  assert.equal(storage.getItem(`${prefix}ops-errors`), '{"route":false}')
  assert.equal(storage.getItem('cpr.accounts.visible-columns'), '{"version":2,"keys":[]}')
  assert.ok(storage.writes.every(([key]) => key.startsWith(prefix)))
})

test('required identity, error and action fields override corrupt preferences and reject hiding', async (t) => {
  for (const [columns, required] of [
    [usageRecordColumns, ['accountEmail', 'actions']],
    [opsErrorColumns, ['accountId', 'message', 'actions']],
  ]) {
    const storage = new MemoryStorage()
    storage.values.set(`${prefix}usage-records`, JSON.stringify(Object.fromEntries(columns.map(column => [column.key, false]))))
    const state = mount(t, columns, 'usage-records', storage)
    assert.deepEqual(keys(state), required)
    for (const key of required) {
      assert.equal(option(state, key).disabled, true)
      state.setColumnVisible(key, false)
    }
    await vue.nextTick()
    assert.deepEqual(keys(state), required)
    assert.deepEqual(storage.writes, [])
    state.resetColumns()
    await vue.nextTick()
    assert.deepEqual(keys(state), Array.from(columns, column => column.key))
  }
})

test('last visible column is protected, removed columns disappear and empty sources remain usable', async (t) => {
  const source = vue.shallowRef([{ key: 'a', label: 'A' }, { key: 'b', label: 'B' }])
  const storage = new MemoryStorage()
  storage.values.set(`${prefix}usage-records`, '{"a":false,"b":false}')
  const state = mount(t, source, 'usage-records', storage)
  assert.deepEqual(keys(state), ['a'])
  assert.equal(option(state, 'a').disabled, true)
  state.setColumnVisible('a', false)
  await vue.nextTick()
  assert.deepEqual(storage.writes, [])
  state.setColumnVisible('b', true)
  assert.equal(option(state, 'a').disabled, false)
  state.setColumnVisible('b', false)
  source.value = [source.value[1]]
  assert.deepEqual(keys(state), ['b'])
  source.value = []
  assert.deepEqual(keys(state), [])
  assert.deepEqual(Array.from(state.columnOptions.value), [])
  state.resetColumns()
  await vue.nextTick()
  assert.deepEqual(state.errors, [])
})

test('storage read and write failures leave in-memory controls usable', async (t) => {
  const storage = {
    getItem: () => { throw new Error('Storage unavailable') },
    setItem: () => { throw new Error('Storage unavailable') },
    removeItem: () => { throw new Error('Storage unavailable') },
  }
  const state = mount(t, usageRecordColumns, 'usage-records', storage)
  assert.equal(option(state, 'model').visible, true)
  state.setColumnVisible('model', false)
  await vue.nextTick()
  assert.equal(option(state, 'model').visible, false)
  state.resetColumns()
  await vue.nextTick()
  assert.equal(option(state, 'model').visible, true)
  assert.ok(state.errors.length >= 1)
})

test('real storage events normalize corruption, synchronize reset and stop after disposal', async (t) => {
  const originals = ['Storage', 'StorageEvent'].map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)])
  Object.defineProperty(globalThis, 'Storage', { configurable: true, value: MemoryStorage })
  Object.defineProperty(globalThis, 'StorageEvent', {
    configurable: true,
    value: class extends Event {
      constructor(type, values) {
        super(type)
        Object.assign(this, values)
      }
    },
  })
  t.after(() => {
    for (const [key, descriptor] of originals) {
      if (descriptor)
        Object.defineProperty(globalThis, key, descriptor)
      else
        delete globalThis[key]
    }
  })
  const window = new EventTarget()
  const state = mount(t, usageRecordColumns, 'usage-records', new MemoryStorage(), window)
  const receive = async (newValue, key = `${prefix}usage-records`) => {
    window.dispatchEvent(new StorageEvent('storage', { key, storageArea: state.storage, newValue }))
    await vue.nextTick()
  }
  await vue.nextTick()
  await receive('{"model":false,"actions":false}')
  assert.equal(option(state, 'model').visible, false)
  assert.equal(option(state, 'actions').visible, true)
  await receive('{"model":true}', `${prefix}ops-errors`)
  assert.equal(option(state, 'model').visible, false)
  for (const raw of ['null', '{broken', '[]']) {
    await receive(raw)
    assert.equal(option(state, 'model').visible, true)
    await receive('{"model":false}')
  }
  await receive(null)
  assert.equal(option(state, 'model').visible, true)
  state.stop()
  await receive('{"model":false}')
  assert.equal(option(state, 'model').visible, true)
  assert.deepEqual(state.errors, [])
})
