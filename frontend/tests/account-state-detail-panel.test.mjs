/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import { createSSRApp, defineComponent, h, ref } from 'vue'
import { compileScript, parse } from 'vue/compiler-sfc'
import { renderToString } from 'vue/server-renderer'

const require = createRequire(import.meta.url)
const modules = new Map()
const fixedNow = new Date('2026-09-18T12:00:00Z')
const icons = {
  ShieldCheck: defineComponent({ setup: () => () => h('svg', { 'data-icon': 'ShieldCheck' }) }),
}

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
    require(name) {
      if (name === '@lucide/vue')
        return icons
      if (name === '@vueuse/core')
        return { createSharedComposable: fn => fn, useNow: () => ref(new Date(fixedNow)) }
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

const component = loadSource(new URL('../src/views/accounts/components/AccountTurnStatePanel.vue', import.meta.url)).default

function account(fields = {}) {
  return {
    id: 'acct-state-panel',
    provider: 'openai',
    turnStateInjectionEnabled: true,
    turnState: {
      requiredModels: ['gpt-6-astra', 'gpt-5.6-sol', 'gpt-5.6-terra'],
      readyModels: [
        { model: 'gpt-6-astra', expiresAt: '2026-09-18T12:50:00Z' },
        { model: 'gpt-5.6-sol', expiresAt: '2026-09-18T12:20:00Z' },
      ],
      models: [
        {
          model: 'gpt-6-astra',
          refreshStatus: 'ready',
          active: { chars: 332, capturedAt: '2026-09-18T11:50:05Z', expiresAt: '2026-09-18T12:50:00Z' },
          probeAttempts: 11,
          successfulProbeAttempt: 2,
          standby: { chars: 332, expiresAt: '2026-09-18T12:55:00Z' },
        },
        {
          model: 'gpt-5.6-sol',
          refreshStatus: 'refreshing',
          active: { chars: 332, expiresAt: '2026-09-18T12:20:00Z' },
          standby: null,
        },
        {
          model: 'gpt-5.6-terra',
          refreshStatus: 'refreshing',
          active: null,
          standby: null,
        },
      ],
    },
    ...fields,
  }
}

function render(fields = {}) {
  return renderToString(createSSRApp(component, { account: account(fields) }))
}

test('probe cooldown preserves ready State and displays deadline, round and lifetime counts', async () => {
  const fixture = account()
  Object.assign(fixture.turnState.models[0], {
    refreshStatus: 'cooldown',
    probeCooldownUntil: '2026-09-18T12:00:50Z',
    probeTotalAttempts: 456,
    probeAttempts: 16,
    probeHttpStatus: 429,
    probeRetryFromUpstream: true,
    probeErrorCode: 'rate_limit_exceeded',
    probeReturnedLength: 356,
    lastProbeReason: 'probe_rate_limited',
  })
  const html = await render(fixture)
  assert.match(html, /可用 · 探测冷却/)
  assert.match(html, /50s 后重试/)
  assert.match(html, /本轮 16 次/)
  assert.match(html, /累计 456 次/)
  assert.match(html, /最近返回 356 字符/)
  assert.match(html, /等待时间来源：上游/)
  assert.match(html, />2\/3<\/strong> 模型/)
  fixture.turnState.models[0].active = null
  fixture.turnState.models[0].standby = null
  const missing = await render(fixture)
  assert.doesNotMatch(missing, /可用 · 探测冷却/)
  assert.match(missing, />1\/3<\/strong> 模型/)
})

test('expired probe cooldown waits for authoritative retry and account errors still override', async () => {
  const fixture = account()
  Object.assign(fixture.turnState.models[2], {
    refreshStatus: 'cooldown',
    probeCooldownUntil: '2026-09-18T11:59:59Z',
  })
  const html = await render(fixture)
  assert.match(html, /等待重试/)
  assert.doesNotMatch(html, /后重试/)
  fixture.credentialState = 'invalid'
  fixture.status = 'error'
  fixture.turnState.models[2].probeCooldownUntil = '2026-09-18T12:01:00Z'
  const blocked = await render(fixture)
  assert.match(blocked, /已停止/)
  assert.doesNotMatch(blocked, /后重试/)
})

test('expanded account State panel lists model slots, lengths, totals, and countdowns', async () => {
  const html = await render()
  assert.match(html, /data-account-turn-state-panel/)
  assert.match(html, />2\/3<\/strong> 模型/)
  assert.match(html, />3<\/strong> 个 State/)
  assert.match(html, /gpt-6-astra/)
  assert.match(html, /gpt-5\.6-sol/)
  assert.match(html, /gpt-5\.6-terra/)
  assert.equal((html.match(/>332 字符 ·/g) ?? []).length, 3)
  assert.match(html, /332 字符 · 50m/)
  assert.match(html, /332 字符 · 55m/)
  assert.match(html, /刷新中/)
  assert.match(html, /等待切换/)
  assert.match(html, /采集时间 2026\/9\/18 19:50:05/)
  assert.match(html, /本轮 11 次/)
  assert.match(html, /第 2 次成功/)
  assert.doesNotMatch(html, /第 11 次成功/)
  assert.doesNotMatch(html, /备用/)
  assert.match(html, /采集中/)
  assert.match(html, /data-icon="ShieldCheck"/)
})

test('State panel explains disabled and unavailable states without inventing captured values', async () => {
  const disabled = await render({ turnStateInjectionEnabled: false })
  assert.match(disabled, /账号级开关已关闭/)
  assert.match(disabled, />3<\/strong> 个 State/)
  assert.match(disabled, />0\/3<\/strong> 模型/)
  assert.match(disabled, /缓存保留/)
  assert.match(disabled, /332 字符 · 50m/)

  const unavailable = await render({ turnState: null })
  assert.match(unavailable, /全局未启用、账号已暂停或状态暂不可用/)
  assert.doesNotMatch(unavailable, /332 字符/)
})

test('non-OpenAI accounts do not render the managed State panel', async () => {
  assert.doesNotMatch(await render({ provider: 'xai' }), /data-account-turn-state-panel/)
})

test('credential errors and paused accounts override historical ready or refreshing slots', async () => {
  for (const [fields, label] of [
    [{ status: 'error', errorReason: 'access_token_expired' }, '等待自动刷新'],
    [{ status: 'error', errorReason: 'credential_expired' }, '需要重新登录'],
    [{ status: 'error', errorReason: 'account_banned' }, '账号已封禁'],
    [{ status: 'quota_exhausted' }, '额度已耗尽'],
    [{ enabled: false }, '账号已暂停'],
  ]) {
    const html = await render(fields)
    assert.match(html, new RegExp(label))
    assert.match(html, /data-account-turn-state-blocked/)
    assert.doesNotMatch(html, /采集中|等待切换|刷新中/)
    assert.match(html, />0\/3<\/strong> 模型/)
  }
  assert.match(await render({ status: 'normal', errorReason: null }), /等待切换/)
})

test('model list retains every model in a bounded region, keyboard-focusable only when overflowing', async () => {
  for (const count of [2, 3, 4, 10]) {
    const models = Array.from({ length: count }, (_, index) => ({
      model: `model-${index}`,
      refreshStatus: 'refreshing',
      active: null,
      standby: null,
    }))
    const html = await render({
      turnState: { requiredModels: models.map(item => item.model), readyModels: [], models },
    })
    assert.equal((html.match(/data-account-turn-state-model/g) ?? []).length, count)
    assert.match(html, /data-account-turn-state-list[^>]*role="region"[^>]*aria-label="模型 State 状态"/)
    assert.match(html, /max-h-60.*overflow-y-auto/)
    assert.match(html, /max-sm:max-h-72/)
    assert.equal(/tabindex="0"/.test(html), count > 3)
    assert.match(html, new RegExp(`>0/${count}</strong> 模型`))
  }
})

test('global disable retains cached countdowns and the last minute is not ready', async () => {
  const state = account().turnState
  const disabled = await render({ turnState: { ...state, enabled: false } })
  assert.match(disabled, /总开关已关闭/)
  assert.match(disabled, />0\/3<\/strong> 模型/)
  assert.match(disabled, /332 字符 · 50m/)
  const cutoff = await render({
    turnState: {
      enabled: true,
      requiredModels: ['model-a'],
      readyModels: [],
      models: [{
        model: 'model-a',
        refreshStatus: 'queued',
        active: { chars: 332, expiresAt: '2026-09-18T12:00:59Z' },
        standby: null,
        probeAttempts: 71,
        lastProbeReason: 'missing_completed',
      }],
    },
  })
  assert.match(cutoff, />0\/1<\/strong> 模型/)
  assert.match(cutoff, /排队中/)
  assert.match(cutoff, /本轮 71 次/)
  assert.doesNotMatch(cutoff, /已就绪/)
})
