/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import { useStorage } from '@vueuse/core'
import ts from 'typescript'
import * as vue from 'vue'

const columns = [
  { key: 'identity', sortable: 'email' },
  { key: 'addedAt', sortable: true },
  { key: 'actions' },
]
const prefix = 'codex-proxy:table-sort:'

function storage() {
  const values = new Map()
  return {
    getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: key => values.delete(key),
  }
}

function mount(t, saved, tableId = 'accounts', window) {
  const exports = {}
  const { outputText } = ts.transpileModule(readFileSync(new URL('../src/composables/useTableSort.ts', import.meta.url), 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  const dependencies = {
    vue,
    '@vueuse/core': { useStorage: (key, defaults, _storage, options) => useStorage(key, defaults, saved, { ...options, window }) },
  }
  runInNewContext(outputText, { exports, require: name => dependencies[name] })
  const scope = vue.effectScope()
  const sort = scope.run(() => exports.useTableSort(columns, tableId))
  t.after(() => scope.stop())
  return { sort, stop: () => scope.stop() }
}

test('sort persists across remount and stays isolated between account and key lists', async (t) => {
  const saved = storage()
  const first = mount(t, saved)
  assert.equal(first.sort.value, undefined)
  assert.equal(saved.getItem(`${prefix}accounts`), null)
  first.sort.value = { key: 'email', direction: 'asc' }
  await vue.nextTick()
  first.stop()
  const reopened = mount(t, saved)
  assert.deepEqual({ ...reopened.sort.value }, { key: 'email', direction: 'asc' })
  assert.equal(mount(t, saved, 'api-keys').sort.value, undefined)
  reopened.sort.value = undefined
  await vue.nextTick()
  assert.equal(saved.getItem(`${prefix}accounts`), null)
})

test('invalid or stale stored sort falls back to existing defaults', (t) => {
  for (const raw of ['{broken', 'null', '[]', '42', '{"key":"identity","direction":"asc"}', '{"key":"actions","direction":"desc"}', '{"key":"addedAt","direction":"sideways"}']) {
    const saved = storage()
    saved.setItem(`${prefix}accounts`, raw)
    assert.equal(mount(t, saved).sort.value, undefined, raw)
  }
})

test('another tab cannot change an active paginated query sort', async (t) => {
  const saved = storage()
  const events = []
  const window = { addEventListener: name => events.push(name), removeEventListener() {} }
  const active = mount(t, saved, 'accounts', window)
  saved.setItem(`${prefix}accounts`, '{"key":"addedAt","direction":"asc"}')
  await vue.nextTick()
  assert.equal(active.sort.value, undefined)
  assert.equal(events.includes('storage'), false)
  assert.equal(mount(t, saved).sort.value.direction, 'asc')
})
