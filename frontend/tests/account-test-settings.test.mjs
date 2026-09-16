/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

const require = createRequire(import.meta.url)
const settingsKey = 'cpr.accounts.connection-test-settings'

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

function mountTest(storage, options = {}) {
  const scope = vue.effectScope()
  let send
  let end
  const requests = []
  let reloads = 0
  let storageRef
  let serializer
  const module = loadModule(new URL('../src/views/accounts/composables/useAccountConnectionTest.ts', import.meta.url), {
    'vue': { ...vue, onBeforeUnmount: vue.onScopeDispose },
    '@vueuse/core': {
      useLocalStorage: (key, defaults, options) => {
        serializer = options.serializer
        storageRef = vue.ref(storage.has(key) ? serializer.read(storage.get(key)) : defaults)
        vue.watch(storageRef, value => storage.set(key, serializer.write(value)), { deep: true, flush: 'sync' })
        return storageRef
      },
    },
    '@/api': {
      getAccountModels: options.getAccountModels ?? (async () => ({ models: [{ id: 'test-model', label: 'Test Model' }] })),
      refreshAccountModels: async () => ({ models: [{ id: 'test-model' }] }),
    },
    '@/api/modules/account-connection-test': {
      streamAccountConnectionTest: (payload, signal, onMessage) => new Promise((resolve) => {
        requests.push(payload)
        const deliver = event => onMessage(JSON.stringify(event))
        send = deliver
        end = resolve
        signal.addEventListener('abort', () => resolve(), { once: true })
        if (options.autoComplete !== false)
          queueMicrotask(() => deliver({ type: 'test_complete', success: true }))
      }),
    },
    '@/components/base/BaseToast': { toast: { success: () => {}, error: assert.fail } },
    '@/composables/useIdSet': {
      useIdSet: () => {
        const ids = new Set()
        return { ids, has: id => ids.has(id), add: id => ids.add(id), remove: id => ids.delete(id) }
      },
    },
    '@/utils/async': { errorMessage: error => error.message, withMinimumDuration: options.withMinimumDuration ?? (task => task()) },
    '@/utils/date': { formatDateTime: () => 'now', formatTime: () => 'now' },
  }, { AbortController, TextEncoder, queueMicrotask })
  const state = scope.run(() => module.useAccountConnectionTest({
    reload: async () => { reloads += 1 },
  }))
  return {
    state,
    requests,
    stop: () => scope.stop(),
    reloads: () => reloads,
    send: event => send(event),
    end: () => end(),
    // Model the serializer path used by VueUse for a cross-tab storage event.
    receiveStorage: (raw) => { storageRef.value = serializer.read(raw) },
  }
}

test('connection test normalizes corrupt and legacy settings before rendering', () => {
  for (const raw of ['null', '[]', 'false', '"old"', '{bad', '{"prompt":17,"stream":"false","endpoint":"completions"}']) {
    const query = mountTest(new Map([[settingsKey, raw]]))
    try {
      assert.equal(query.state.connectionTestPrompt.value, 'Reply with exactly OK.')
      assert.equal(query.state.connectionTestStream.value, true)
      query.receiveStorage('null')
      assert.equal(query.state.connectionTestPrompt.value, 'Reply with exactly OK.')
      query.receiveStorage('{"prompt":"新的提示","stream":false,"endpoint":"completions"}')
      assert.equal(query.state.connectionTestPrompt.value, '新的提示')
      assert.equal(query.state.connectionTestStream.value, false)
    }
    finally {
      query.stop()
    }
  }
})

test('connection test persists preferences across accounts and reloads and sends exact Unicode input', async () => {
  const storage = new Map([[settingsKey, '{"endpoint":"completions","prompt":"old","stream":true}']])
  const query = mountTest(storage)
  const prompt = '请只回答 OK\n保留 + & ? # 和空格\t'
  try {
    query.state.openConnectionTest({ id: 'account-a' })
    await new Promise(resolve => setImmediate(resolve))
    query.state.connectionTestPrompt.value = prompt
    query.state.connectionTestStream.value = false
    await query.state.handleTestConnection()
    assert.equal(query.requests.length, 1, query.state.connectionTestError.value)
    assert.equal(query.requests[0].prompt, prompt)
    assert.equal(query.requests[0].stream, false)
    assert.equal(query.requests[0].interface, 'responses')
    assert.equal(query.reloads(), 1)
    query.state.openConnectionTest({ id: 'account-b' })
    await new Promise(resolve => setImmediate(resolve))
    assert.equal(query.state.connectionTestPrompt.value, prompt)
    await query.state.handleTestConnection()
    assert.equal(query.requests[1].accountId, 'account-b')
  }
  finally {
    query.stop()
  }
  const reopened = mountTest(storage)
  try {
    assert.equal(reopened.state.connectionTestPrompt.value, prompt)
    assert.equal(reopened.state.connectionTestStream.value, false)
  }
  finally {
    reopened.stop()
  }
})

test('invalid prompts never open an upstream test; byte boundaries and multiline input are accepted', async () => {
  const query = mountTest(new Map())
  try {
    query.state.openConnectionTest({ id: 'account-a' })
    await new Promise(resolve => setImmediate(resolve))
    for (const [prompt, expected] of [
      [' \n\t', '请先填写'],
      ['中'.repeat(1366), '4096 字节'],
      ['hello\x00world', '控制字符'],
      ['hello\x85world', '控制字符'],
    ]) {
      query.state.connectionTestPrompt.value = prompt
      await query.state.handleTestConnection()
      assert.ok(query.state.connectionTestError.value.includes(expected), query.state.connectionTestError.value)
      assert.equal(query.requests.length, 0)
    }
    for (const prompt of [`${'中'.repeat(1365)}a`, 'a'.repeat(4096), 'a\nb\rc\td']) {
      query.state.connectionTestPrompt.value = prompt
      await query.state.handleTestConnection()
      assert.equal(query.state.connectionTestStatus.value, 'success')
      assert.equal(query.state.connectionTestError.value, '')
      assert.equal(query.requests.at(-1).prompt, prompt)
    }
  }
  finally {
    query.stop()
  }
})

test('saved column choices migrate once, adding relogin count without revealing old hidden columns', () => {
  const columns = loadModule(new URL('../src/views/accounts/constants.ts', import.meta.url), {
    '@/components/base/BaseTable/columns': { defineTableColumns: value => value },
  })
  const read = raw => Array.from(columns.readAccountColumnKeys(raw))
  assert.deepEqual(read('["identity","addedAtDisplay","provider","accountId","health",null]'), ['identity', 'addedAt', 'health', 'reloginCount'])
  assert.deepEqual(read('["identity"]'), ['identity', 'reloginCount'])
  assert.deepEqual(read('[]'), ['reloginCount'])
  assert.deepEqual(read('["addedAt","addedAtDisplay"]'), ['addedAt', 'reloginCount'])
  assert.deepEqual(read(columns.writeAccountColumnKeys(['identity'])), ['identity'])
  assert.deepEqual(read(columns.writeAccountColumnKeys([])), [])
  assert.deepEqual(read(columns.writeAccountColumnKeys(['identity', 'reloginCount'])), ['identity', 'reloginCount'])
  for (const raw of ['null', '{}', '"old"', 'broken']) {
    assert.ok(read(raw).includes('addedAt'))
    assert.ok(!read(raw).includes('accountId'))
  }
})

test('an old model request cannot replace the newly selected account model list', async () => {
  const pending = new Map()
  const query = mountTest(new Map(), {
    getAccountModels: ({ accountId }) => new Promise(resolve => pending.set(accountId, resolve)),
  })
  try {
    query.state.openConnectionTest({ id: 'account-a' })
    query.state.openConnectionTest({ id: 'account-b' })
    pending.get('account-b')({ models: [{ id: 'model-b' }] })
    await new Promise(resolve => setImmediate(resolve))
    assert.equal(query.state.connectionTestSelectedModel.value, 'model-b')
    pending.get('account-a')({ models: [{ id: 'model-a' }] })
    await new Promise(resolve => setImmediate(resolve))
    assert.equal(query.state.connectionTestSelectedModel.value, 'model-b')
  }
  finally {
    query.stop()
  }
})

test('finishing a cancelled test cannot fail or close the next account test', async () => {
  const completions = []
  const query = mountTest(new Map(), {
    autoComplete: false,
    withMinimumDuration: async (task) => {
      await task()
      await new Promise(resolve => completions.push(resolve))
    },
  })
  try {
    query.state.openConnectionTest({ id: 'account-a' })
    await new Promise(resolve => setImmediate(resolve))
    const previous = query.state.handleTestConnection()
    query.state.openConnectionTest({ id: 'account-b' })
    await new Promise(resolve => setImmediate(resolve))
    const current = query.state.handleTestConnection()
    completions.shift()()
    await previous
    assert.equal(query.state.connectionTestStatus.value, 'running')
    query.send({ type: 'test_complete', success: true })
    await new Promise(resolve => setImmediate(resolve))
    completions.shift()()
    await current
    assert.equal(query.state.connectionTestStatus.value, 'success')
  }
  finally {
    query.stop()
    for (const resolve of completions)
      resolve()
  }
})

test('premature stream closure and malformed events cannot produce an empty success', async () => {
  for (const malformed of [false, true]) {
    const query = mountTest(new Map(), { autoComplete: false })
    try {
      query.state.openConnectionTest({ id: 'account-a' })
      await new Promise(resolve => setImmediate(resolve))
      const result = query.state.handleTestConnection()
      if (malformed)
        query.send({ unexpected: 'not an event' })
      else
        query.end()
      await result
      assert.equal(query.state.connectionTestStatus.value, 'error')
      assert.match(query.state.connectionTestError.value, malformed ? /解析失败/ : /未返回完成事件/)
      assert.equal(query.state.testingConnectionIds.has('account-a'), false)
    }
    finally {
      query.stop()
    }
  }
})

test('upstream authentication failures tell the administrator how to recover', async () => {
  const query = mountTest(new Map(), { autoComplete: false })
  try {
    query.state.openConnectionTest({ id: 'account-a' })
    await new Promise(resolve => setImmediate(resolve))
    const result = query.state.handleTestConnection()
    query.send({
      type: 'error',
      source: 'upstream',
      gatewayErrorCode: 'unauthorized',
      upstreamStatus: 401,
    })
    await result
    assert.match(query.state.connectionTestError.value, /刷新 Token/)
    assert.match(query.state.connectionTestError.value, /重新授权/)
  }
  finally {
    query.stop()
  }
})

test('model catalog failures are visible before the connection test starts', async () => {
  const query = mountTest(new Map(), {
    getAccountModels: async () => {
      throw new Error('OpenAI 拒绝了用于获取模型列表的账号凭据。请先刷新 Token；仍失败再重新授权。')
    },
  })
  try {
    query.state.openConnectionTest({ id: 'account-a' })
    await new Promise(resolve => setImmediate(resolve))
    assert.match(query.state.connectionTestError.value, /模型列表加载失败/)
    assert.match(query.state.connectionTestError.value, /刷新 Token/)
    assert.equal(query.state.connectionTestStatus.value, 'idle')
    assert.equal(query.requests.length, 0)
  }
  finally {
    query.stop()
  }
})
