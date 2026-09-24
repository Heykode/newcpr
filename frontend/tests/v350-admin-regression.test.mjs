/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import axios from 'axios'
import ts from 'typescript'
import * as vue from 'vue'
import { compileScript, parse } from 'vue/compiler-sfc'
import { renderToString } from 'vue/server-renderer'

const require = createRequire(import.meta.url)
const source = path => new URL(`../src/${path}`, import.meta.url)
const tick = () => new Promise(resolve => setImmediate(resolve))

function loadText(text, dependencies = {}, globals = {}) {
  const exports = {}
  const { outputText } = ts.transpileModule(text, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, {
    exports,
    require: name => dependencies[name] ?? require(name),
    AbortController,
    ...globals,
  })
  return exports
}

function load(path, dependencies = {}, globals = {}) {
  return loadText(readFileSync(source(path), 'utf8'), dependencies, globals)
}

function requestHarness() {
  const messages = []
  const errors = load('api/error.ts')
  const notifications = { toast: { error: text => messages.push(text), success: () => {}, warning: () => {} } }
  const api = load('api/request.ts', {
    './constants': { API_BASE_URL: '', API_TIMEOUT_MS: 1000 },
    './error': errors,
    '@/components/base/BaseToast': notifications,
  })
  const actions = load('composables/useAsyncAction.ts', {
    vue,
    '@/api/request': errors,
    '@/components/base/BaseToast': notifications,
    '@/utils/async': { errorMessage: error => error.message, withMinimumDuration: task => task() },
  })
  const response = (config, data, status = 200) => ({
    config,
    data,
    status,
    statusText: '',
    headers: { 'x-request-id': 'test-request' },
  })
  const fail = status => config => Promise.reject(new axios.AxiosError(
    'transport failure',
    'ERR_BAD_RESPONSE',
    config,
    undefined,
    response(config, { code: 40001, message: 'synthetic failure' }, status),
  ))
  return { ...api, ...errors, ...actions, messages, response, fail }
}

test('request notifications handle business errors once, preserve silent auth handling and suppress cancellation', async () => {
  const api = requestHarness()
  await assert.rejects(
    api.default({ url: '/api/admin/accounts', adapter: config => Promise.resolve(api.response(config, { code: 40901, message: 'revision conflict', data: null })) }),
    error => error instanceof api.ApiError && error.code === 40901 && error.requestId === 'test-request',
  )
  assert.deepEqual(api.messages, ['revision conflict'])
  const action = api.useAsyncAction()
  await action.run(() => api.default({ url: '/api/admin/accounts', adapter: api.fail(503) }))
  assert.deepEqual(api.messages, ['revision conflict', 'synthetic failure'], 'the action must not duplicate the API toast')
  await action.run(() => {
    throw new Error('local validation')
  })
  assert.equal(api.messages.at(-1), 'local validation', 'local validation must remain visible')

  let expired = 0
  api.setUnauthorizedHandler(() => expired += 1)
  const count = api.messages.length
  for (let attempt = 0; attempt < 2; attempt += 1) {
    await assert.rejects(api.default({ url: '/api/admin/settings', silent: true, adapter: api.fail(401) }))
  }
  assert.equal(expired, 1)
  assert.equal(api.messages.length, count, 'background failures stay silent without swallowing auth expiry')
  const controller = new AbortController()
  controller.abort()
  await assert.rejects(api.default({ url: '/api/admin/accounts', signal: controller.signal }))
  assert.equal(api.messages.length, count)
  assert.equal(await api.default({
    url: '/api/admin/settings',
    adapter: config => Promise.resolve(api.response(config, { code: 200, message: 'OK', data: 42 })),
  }), 42)
})

test('custom IPv6, UA and runtime APIs forward silent cancellation options without changing payloads', async () => {
  const calls = []
  const options = { silent: true, timeout: 2500, signal: new AbortController().signal }
  const dependency = { '../request': async (config) => {
    calls.push(config)
  } }
  const ipv6 = load('api/modules/ipv6-egress.ts', dependency)
  const ua = load('api/modules/outbound-user-agent.ts', dependency)
  const settings = load('api/modules/settings.ts', dependency)
  const addresses = [{ id: 'source-test', address: '2001:db8::1', enabled: true }]
  const payload = { revision: 4, defaultMode: 'unchanged', addresses }
  const tuning = { requestTuning: { websocketMaxConnecting: 24 }, rotationStrategy: 'smart' }
  await ipv6.getIpv6Egress(options)
  await ipv6.updateIpv6Egress(payload, options)
  await ipv6.updateAccountIpv6Egress({ accountId: 'account-test', revision: 4, mode: null }, options)
  await ipv6.expandIpv6Egress({ start: '2001:db8::1', end: '2001:db8::2' }, options)
  await ua.getOutboundUserAgent(options)
  await ua.previewOutboundUserAgent({ mode: 'custom', userAgent: 'test' }, options)
  await ua.updateOutboundUserAgent({ mode: 'default' }, options)
  await settings.getSettings(options)
  await settings.updateSettings(tuning, options)
  await settings.getCodexDesktopWindowsDownloads(true, options)
  assert.equal(calls.length, 10)
  for (const config of calls) {
    assert.equal(config.silent, true)
    assert.equal(config.signal, options.signal)
    assert.equal(config.timeout, 2500)
  }
  assert.equal(calls[1].data, payload, 'saving a page must retain the entire source pool')
  assert.equal(calls[2].data.mode, null, 'inherit remains distinct from unchanged')
  assert.equal(calls[8].data, tuning, 'custom runtime parameters remain intact')
})

test('outbound UA automatic refresh remains silent while manual reload errors remain visible', async () => {
  const filename = source('views/settings/components/OutboundUserAgentCard.vue')
  const { descriptor } = parse(readFileSync(filename, 'utf8'), { filename: filename.pathname })
  const compiled = compileScript(descriptor, { id: 'outbound-ua-test' })
  const mounted = []
  let poll
  let failing = false
  const requests = []
  const settings = { mode: 'custom', customUserAgent: 'custom-profile', defaultUserAgent: 'default-profile' }
  const component = loadText(compiled.content, {
    'vue': { ...vue, onMounted: callback => mounted.push(callback), onUnmounted: vue.onScopeDispose },
    '@vueuse/core': { useIntervalFn: (callback, delay) => {
      assert.equal(delay, 30_000)
      poll = callback
    } },
    '@/api/modules/outbound-user-agent': {
      getOutboundUserAgent: async (options) => {
        requests.push(options)
        if (failing)
          throw new Error('synthetic failure')
        return settings
      },
    },
    '@/components/base/BaseButton.vue': {},
    '@/components/base/BaseCard.vue': {},
    '@/components/base/BaseCheckbox.vue': {},
    '@/components/base/BaseForm/FormItem.vue': {},
    '@/components/base/BaseSelect.vue': {},
    '@/components/base/BaseTextarea.vue': {},
    '@/components/base/BaseToast': { toast: { success: () => {} } },
    '@/utils/async': { errorMessage: error => error.message },
    './outbound-user-agent-samples': load('views/settings/components/outbound-user-agent-samples.ts', {
      './outbound-user-agent-samples.json': JSON.parse(readFileSync(source('views/settings/components/outbound-user-agent-samples.json'), 'utf8')),
    }),
  })
  const scope = vue.effectScope()
  try {
    const state = scope.run(() => component.default.setup({}, { expose: () => {} }))
    mounted[0]()
    await tick()
    assert.equal(state.custom.value, 'custom-profile')
    assert.equal(requests[0].silent, false)
    failing = true
    poll()
    await tick()
    assert.equal(requests.at(-1).silent, true)
    assert.equal(state.error.value, '', 'a background poll must not overwrite the editable UA form with an error')
    await state.load(true)
    assert.equal(requests.at(-1).silent, false)
    assert.equal(state.error.value, 'synthetic failure')
    assert.equal(state.custom.value, 'custom-profile')
  }
  finally {
    scope.stop()
  }
})

test('runtime save keeps custom parameters and silently rereads server state after a rejected mutation', async () => {
  const api = requestHarness()
  const loads = []
  const updates = []
  const server = {
    refreshMarginSeconds: 60,
    refreshConcurrency: 8,
    maxConcurrentPerAccount: 20,
    requestIntervalMs: 0,
    rotationStrategy: 'smart',
    modelMappings: {},
    usageRetentionDays: 31,
    opsEventRetentionDays: 30,
    auditRetentionDays: 90,
    requestTuning: { maxRequestAttempts: 24, websocketHttpFallbackEnabled: false },
  }
  const settings = load('views/settings/composables/useSettingsForm.ts', {
    vue,
    '@/api': {
      getSettings: async (options) => {
        loads.push(options)
        return server
      },
      updateSettings: async (payload) => {
        updates.push(payload)
        throw new api.ApiError('conflict', 409)
      },
    },
    '@/api/request': api,
    '@/components/base/BaseToast': { toast: { warning: assert.fail, success: assert.fail } },
    '@/composables/useAsyncAction': api,
    '@/utils/async': { errorMessage: error => error.message },
  })
  const state = settings.useSettingsForm()
  await state.loadSettings()
  state.form.requestTuning.maxRequestAttempts = 32
  await state.saveSettings()
  await tick()
  assert.equal(updates[0].requestTuning.maxRequestAttempts, 32)
  assert.equal(Object.hasOwn(updates[0].requestTuning, 'websocketMaxConnecting'), false)
  assert.equal(updates[0].requestTuning.websocketHttpFallbackEnabled, false)
  assert.equal(loads.at(-1).silent, true)
  assert.equal(state.form.requestTuning.maxRequestAttempts, 24)
  assert.equal(Object.hasOwn(state.form.requestTuning, 'websocketMaxConnecting'), false)
  assert.equal(state.saving.value, false)
})

test('reset-credit unknown results and in-flight lock survive closing and reopening the account', async () => {
  const api = requestHarness()
  const sent = []
  let rejectConsumption
  const credit = { id: 'credit-test', status: 'available' }
  const module = load('views/accounts/composables/useAccountResetCredits.ts', {
    vue,
    '@/api': {
      getAccountResetCredits: async () => ({ credits: [credit], availableCount: 1 }),
      consumeAccountResetCredit: (payload, options) => {
        sent.push({ payload, options })
        if (sent.length === 1)
          return new Promise((_, reject) => rejectConsumption = reject)
        return Promise.resolve({ code: 'already_redeemed' })
      },
      refreshAccountQuota: async () => ({ account: { id: 'account-test' } }),
    },
    '@/api/request': api,
    '@/components/base/BaseToast': { toast: { success: () => {}, error: assert.fail, warning: () => {} } },
    '@/utils/async': { errorMessage: error => error.message },
  }, { crypto: globalThis.crypto })
  const firstScope = vue.effectScope()
  let oldUpdates = 0
  const first = firstScope.run(() => module.useAccountResetCredits({
    accountId: () => 'account-test',
    onAccountUpdated: () => oldUpdates += 1,
  }))
  await first.loadCredits()
  first.selectCredit(credit.id)
  first.requestConsume()
  const inFlight = first.confirmConsume()
  assert.equal(sent[0].options.silent, true)
  firstScope.stop()
  const secondScope = vue.effectScope()
  let newUpdates = 0
  try {
    const second = secondScope.run(() => module.useAccountResetCredits({
      accountId: () => 'account-test',
      onAccountUpdated: () => newUpdates += 1,
    }))
    assert.equal(second.consuming.value, true)
    second.requestConsume()
    assert.equal(await second.confirmConsume(), false)
    assert.equal(sent.length, 1, 'a remount cannot submit a second concurrent redemption')
    rejectConsumption(new api.ApiError('connection lost', 0, undefined, undefined, 'network'))
    await inFlight
    assert.equal(second.ambiguous.value, true)
    second.requestConsume()
    assert.equal(await second.confirmConsume(), true)
    assert.equal(sent[1].payload.redeemRequestId, sent[0].payload.redeemRequestId)
    assert.equal(oldUpdates, 0)
    assert.equal(newUpdates, 1)
  }
  finally {
    secondScope.stop()
  }
})

test('quota forecast is loaded only on demand and cannot replace another account or closed modal', async () => {
  const pending = []
  let refreshes = 0
  const module = load('views/accounts/composables/useAccountQuotaForecast.ts', {
    vue,
    '@/api': {
      getAccountQuotaForecast: (params, options) => new Promise(resolve => pending.push({ ...params, ...options, resolve })),
      refreshAccountQuota: async ({ accountId }) => {
        refreshes += 1
        return { account: { id: accountId } }
      },
    },
    '@/components/base/BaseToast': { toast: { success: () => {} } },
  })
  const accountId = vue.ref('account-a')
  const open = vue.ref(false)
  const scope = vue.effectScope()
  const updated = []
  try {
    const state = scope.run(() => module.useAccountQuotaForecast(accountId, open, account => updated.push(account.id)))
    assert.equal(pending.length, 0)
    open.value = true
    await vue.nextTick()
    assert.equal(pending.length, 1)
    accountId.value = 'account-b'
    await vue.nextTick()
    assert.equal(pending.length, 2)
    assert.equal(pending[0].signal.aborted, true)
    pending[1].resolve({ accountId: 'account-b', forecasts: [] })
    await tick()
    pending[0].resolve({ accountId: 'account-a', forecasts: [] })
    await tick()
    assert.equal(state.report.value.accountId, 'account-b')
    assert.equal(refreshes, 0, 'reading the forecast must not refresh quota or call any model')
    const refresh = state.refresh()
    await tick()
    assert.equal(refreshes, 1)
    assert.deepEqual(updated, ['account-b'])
    pending[2].resolve({ accountId: 'account-b', forecasts: [] })
    await refresh
    const closingLoad = state.load()
    open.value = false
    await vue.nextTick()
    assert.equal(pending[3].signal.aborted, true)
    pending[3].resolve({ accountId: 'account-b', forecasts: [] })
    await closingLoad
    assert.equal(state.report.value, null)
    const cell = readFileSync(source('views/accounts/components/AccountQuotaSummaryCell/index.vue'), 'utf8')
    assert.doesNotMatch(cell, /estimatedTotalCost|cost\s*\*\s*100|getAccountQuotaForecast|useIntervalFn/)
    assert.match(cell, /forecastRequested/)
  }
  finally {
    scope.stop()
  }
})

test('forecast capacity preserves partial estimates, keeps unknowns distinct and renders unavailable modal state', async () => {
  const sfc = (path, dependencies = {}) => {
    const filename = source(path)
    const { descriptor } = parse(readFileSync(filename, 'utf8'), { filename: filename.pathname })
    const compiled = compileScript(descriptor, { id: 'forecast-render-test', inlineTemplate: true })
    return loadText(compiled.content, dependencies)
  }
  const component = sfc('views/accounts/components/AccountQuotaForecastModal/ForecastCapacity.vue')
  const base = {
    period: 'weekly',
    source: {
      label: '周额度',
      usedPercent: 25,
      usedPercentDisplay: '25%',
      tokensDisplay: '10K',
      usdDisplay: '$123,456,789,012,345.67',
      resetAt: '2026-09-14T00:00:00Z',
    },
    incompleteCost: false,
    incompleteTokens: false,
    unavailableReason: null,
    estimatedTokensDisplay: '40K',
    estimatedUsdDisplay: '$493,827,156,049,382.68',
    remainingTokensDisplay: '30K',
    remainingUsdDisplay: '$370,370,367,037,037.01',
  }
  const render = forecast => renderToString(vue.createSSRApp(component.default, { forecast }))
  for (const period of ['weekly', 'monthly']) {
    for (const [incompleteCost, incompleteTokens] of [[false, false], [true, false], [false, true], [true, true]]) {
      const html = await render({ ...base, period, incompleteCost, incompleteTokens })
      const notice = /按已记录数据估算，缺失的用量或费用可能使结果偏低/
      if (incompleteCost || incompleteTokens)
        assert.match(html, notice)
      else
        assert.doesNotMatch(html, notice)
      for (const value of ['40K', '30K', '$493,827,156,049,382.68', '$370,370,367,037,037.01'])
        assert.ok(html.includes(value), value)
      assert.match(html, /\$123,456,789,012,345\.67/)
      assert.doesNotMatch(html, /暂时无法预测|\$0\.00/)
      assert.match(html, /minmax\(0,1fr\)_20px_minmax\(0,1\.2fr\)/)
      assert.match(html, /wrap-anywhere/)
    }
  }
  const missingCosts = {
    ...base,
    incompleteCost: true,
    source: { ...base.source, usdDisplay: '—' },
    estimatedUsdDisplay: '—',
    remainingUsdDisplay: '—',
  }
  const missingCostHtml = await render(missingCosts)
  assert.match(missingCostHtml, /40K/)
  assert.match(missingCostHtml, /30K/)
  assert.doesNotMatch(missingCostHtml, /\$/)
  const missingTokens = {
    ...base,
    incompleteTokens: true,
    source: { ...base.source, tokensDisplay: '—' },
    estimatedTokensDisplay: '—',
    remainingTokensDisplay: '—',
  }
  const missingTokenHtml = await render(missingTokens)
  assert.doesNotMatch(missingTokenHtml, /10K|30K|40K/)
  assert.match(missingTokenHtml, /\$493,827,156,049,382\.68/)
  const unknown = {
    ...missingCosts,
    incompleteTokens: true,
    source: { ...missingCosts.source, tokensDisplay: '—' },
    estimatedTokensDisplay: '—',
    remainingTokensDisplay: '—',
    unavailableReason: '本周期暂无可用于估算的 Token 或费用数据，请积累用量后重试。',
  }
  const unknownHtml = await render(unknown)
  assert.doesNotMatch(unknownHtml, /\$|10K|30K|40K|>≈</)
  const report = vue.ref({ forecasts: [base] })
  const slotContainer = { setup: (_, { slots }) => () => vue.h('section', [slots.default?.(), slots.footer?.()]) }
  const empty = { render: () => null }
  const modal = sfc('views/accounts/components/AccountQuotaForecastModal/index.vue', {
    '@vueuse/core': { useNow: () => ({ now: vue.ref(new Date('2026-09-13T00:00:00Z')), pause: () => {}, resume: () => {} }) },
    '@/components/base/BaseButton.vue': slotContainer,
    '@/components/base/BaseEmpty.vue': sfc('components/base/BaseEmpty.vue'),
    '@/components/base/BaseModal/index.vue': slotContainer,
    '@/components/base/BasePopover.vue': slotContainer,
    '@/components/base/BaseSegmented.vue': empty,
    '../../composables/useAccountQuotaForecast': {
      useAccountQuotaForecast: () => ({ report, loading: vue.ref(false), refreshing: vue.ref(false), error: vue.ref(null), load: () => {}, refresh: () => {} }),
    },
    '../AccountIdentityCell.vue': empty,
    './ForecastCapacity.vue': component,
    './Skeleton.vue': empty,
  })
  for (const forecast of [{ ...base, incompleteCost: true, incompleteTokens: true }, unknown]) {
    report.value = { forecasts: [forecast] }
    const html = await renderToString(vue.createSSRApp(modal.default, {
      modelValue: true,
      account: { id: 'account-test', planTypeDisplay: 'Plus' },
    }))
    if (forecast.unavailableReason) {
      assert.match(html, /暂时无法预测/)
      assert.ok(html.includes(forecast.unavailableReason))
      assert.doesNotMatch(html, /aria-label="容量预测"|\$|40K/)
    }
    else {
      assert.match(html, /aria-label="容量预测"/)
      assert.match(html, /40K/)
      assert.doesNotMatch(html, /暂时无法预测/)
    }
    assert.match(html, /不是实际账单或账户余额/)
  }
})

test('diagnostic exports allow only safe fields and never borrow correlation from another request', () => {
  const presentation = load('views/usage/utils/opsErrorPresentation.ts')
  const { requestDiagnosticsBundle } = load('views/usage/utils/diagnosticsBundle.ts', { './opsErrorPresentation': presentation })
  const traceEvent = {
    sequence: 1,
    stage: 'upstream_open',
    get data() {
      assert.fail('must not inspect arbitrary trace data')
    },
  }
  const detail = {
    requestId: 'request-b',
    upstreamRequestId: null,
    message: 'sensitive message',
    metadata: { apiKey: 'sensitive credential', turnState: { injectedState: 'sensitive-state' } },
    trace: { schemaVersion: 1, events: [traceEvent] },
    attempts: [{ attemptIndex: 1, accountId: 'account-test', rawBody: 'sensitive body' }],
    relatedRequests: [],
  }
  const error = { requestId: 'request-a', upstreamRequestId: 'unrelated-id', metadata: {}, message: 'sensitive error' }
  const bundle = requestDiagnosticsBundle('request-b', detail, error)
  assert.equal(bundle.error, null)
  assert.equal(bundle.request.upstreamRequestId, null)
  assert.equal(bundle.trace.events[0].stage, 'upstream_open')
  assert.doesNotMatch(JSON.stringify(bundle), /sensitive|unrelated-id/)
  const unrelated = requestDiagnosticsBundle('request-c', detail, error)
  assert.equal(unrelated.trace, null)
  assert.equal(unrelated.request.source, 'request_id_only')

  const matchingError = {
    ...error,
    requestId: 'request-b',
    failureClass: 'upstream_unavailable',
    providerErrorCode: 'sensitive_upstream_code',
  }
  const matchingDetail = {
    ...detail,
    attempts: [{ failureClass: 'upstream_unavailable', providerErrorCode: 'sensitive_attempt_code' }],
  }
  const safe = requestDiagnosticsBundle('request-b', matchingDetail, matchingError)
  assert.equal(safe.error.summary, 'upstream_unavailable')
  assert.equal(safe.error.summarySource, 'failure_class')
  assert.doesNotMatch(JSON.stringify(safe), /sensitive|providerErrorCode/)
})

test('usage searches preserve email and request IDs but never send full CPR API keys', () => {
  const { usageSearchParam } = load('views/usage/utils/search.ts')
  assert.equal(usageSearchParam('  example@test.invalid  '), 'example@test.invalid')
  assert.equal(usageSearchParam('  request-123  '), 'request-123')
  assert.equal(usageSearchParam('sk_1234567_private_secret_material'), 'sk_1234567')
  assert.equal(usageSearchParam('  '), undefined)
})

test('copy helper enables the existing legacy fallback without changing empty or success feedback', async () => {
  const copied = []
  const success = []
  const module = load('composables/useCopyText.ts', {
    '@vueuse/core': { useClipboard: (options) => {
      assert.equal(options.legacy, true)
      return { copy: async value => copied.push(value) }
    } },
    '@/components/base/BaseToast': { toast: { error: assert.fail, success: value => success.push(value) } },
    '@/utils/async': { errorMessage: error => error.message },
  })
  const copy = module.useCopyText()
  await copy('', { successText: 'copied' })
  await copy('synthetic text', { successText: 'copied' })
  assert.deepEqual(copied, ['synthetic text'])
  assert.deepEqual(success, ['copied'])
})
