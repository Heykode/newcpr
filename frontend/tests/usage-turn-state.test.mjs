/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import { createSSRApp, defineComponent, effectScope, h, nextTick, ref } from 'vue'
import { compileScript, parse } from 'vue/compiler-sfc'
import { renderToString } from 'vue/server-renderer'

const require = createRequire(import.meta.url)
const calls = []
function getUsageRecordDetail(query, options) {
  return new Promise((resolve, reject) => calls.push({ ...query, ...options, resolve, reject }))
}
const modules = new Map()
function loadSource(filename) {
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
    require(name) {
      if (name === '@/api')
        return { getUsageRecordDetail }
      if (name === '@lucide/vue')
        return { RefreshCw: defineComponent({ setup: () => () => h('svg') }) }
      if (name === '@/components/base/BasePopover.vue')
        return defineComponent({ setup: (_, { slots }) => () => h('div', slots.trigger?.()) })
      if (name === '@/components/base/BaseIconButton.vue')
        return defineComponent({ setup: (_, { slots }) => () => h('button', slots.default?.()) })
      if (name.startsWith('@/') || name.startsWith('.')) {
        const path = name.endsWith('.vue') ? name : `${name}.ts`
        return loadSource(name.startsWith('@/')
          ? new URL(`../src/${path.slice(2)}`, import.meta.url)
          : new URL(path, filename))
      }
      return require(name)
    },
  }, { filename: filename.pathname })
  return exports
}
const { useUsageTurnState } = loadSource(new URL('../src/views/usage/composables/useUsageTurnState.ts', import.meta.url))
const component = loadSource(new URL('../src/views/usage/components/UsageTurnStateCell.vue', import.meta.url)).default

function harness() {
  calls.length = 0
  const id = ref('request-a')
  const open = ref(false)
  const scope = effectScope()
  const state = scope.run(() => useUsageTurnState(() => id.value, () => open.value))
  return { id, open, scope, state }
}
function detail(id, value) {
  return { requestId: id, metadata: { turnState: { injectedState: value } } }
}
async function flush() {
  await nextTick()
  await Promise.resolve()
}

test('full State is fetched on demand only and cleared on close and scope disposal', async () => {
  const { id, open, scope, state } = harness()
  try {
    await flush()
    assert.equal(calls.length, 0)
    open.value = true
    await flush()
    assert.equal(calls.length, 1)
    calls[0].resolve(detail(id.value, 'synthetic-state'))
    await flush()
    assert.equal(state.value.value, 'synthetic-state')
    open.value = false
    await flush()
    assert.equal(calls[0].signal.aborted, true)
    assert.equal(state.value.value, null)
    open.value = true
    await flush()
    assert.equal(calls.length, 2, 'do not retain sensitive values between popovers')
    calls[1].resolve(detail(id.value, 'synthetic-new-state'))
    await flush()
    scope.stop()
    assert.equal(state.value.value, null)
    assert.equal(calls[1].signal.aborted, true)
  }
  finally {
    scope.stop()
  }
})

test('late State detail or failure cannot replace another request or a closed popover', async () => {
  const { id, open, scope, state } = harness()
  try {
    open.value = true
    await flush()
    const old = calls[0]
    id.value = 'request-b'
    await flush()
    assert.equal(old.signal.aborted, true)
    calls[1].resolve(detail('request-b', 'state-b'))
    await flush()
    old.resolve(detail('request-a', 'state-a'))
    await flush()
    assert.equal(state.value.value, 'state-b')
    state.retry()
    await flush()
    open.value = false
    await flush()
    calls[2].reject(new Error('synthetic failure'))
    await flush()
    assert.equal(state.value.value, null)
    assert.equal(state.failed.value, false)
  }
  finally {
    scope.stop()
  }
})

test('detail failures are retryable and mismatched, absent or oversized values stay unavailable', async () => {
  const { open, scope, state } = harness()
  try {
    open.value = true
    await flush()
    calls.at(-1).reject(new Error('synthetic failure'))
    await flush()
    assert.equal(state.failed.value, true)
    for (const response of [detail('other-id', 'state'), detail('request-a', 'x'.repeat(2049)), detail('request-a', null)]) {
      state.retry()
      await flush()
      calls.at(-1).resolve(response)
      await flush()
      assert.equal(state.value.value, null)
      assert.equal(state.failed.value, false)
      assert.equal(state.loading.value, false)
    }
  }
  finally {
    scope.stop()
  }
})

test('State cell separates missing, uninjected, same, changed and absent response facts', async () => {
  const render = turnState => renderToString(createSSRApp(component, { record: { id: 'request-a', turnState } }))
  assert.match(await render(undefined), /未记录/)
  assert.match(await render({ injected: false }), /未附加托管 State/)
  const summary = { injected: true, preview: 'syntheti...-state', chars: 332, returnedChars: 332, returnedSame: true, transport: 'http_sse' }
  const same = await render(summary)
  assert.match(same, /syntheti\.\.\.-state/)
  assert.match(same, /332 字符 · 回 332/)
  assert.doesNotMatch(same, /text-cp-warning-text|正常|有效/)
  assert.match(await render({ ...summary, returnedChars: 356, returnedSame: false }), /text-cp-warning-text/)
  assert.match(await render({ ...summary, returnedChars: null, returnedSame: null }), /无回显/)
})
