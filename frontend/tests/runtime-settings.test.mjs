/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

const require = createRequire(import.meta.url)

function loadModule(filename, dependencies = {}) {
  const { outputText } = ts.transpileModule(readFileSync(filename, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  const exports = {}
  runInNewContext(outputText, {
    URL,
    exports,
    require: name => dependencies[name] ?? (name === '@/utils/excel-defaults' ? loadModule(new URL('../src/utils/excel-defaults.ts', import.meta.url)) : require(name)),
  }, { filename: String(filename) })
  return exports
}

const apiErrors = loadModule(new URL('../src/api/error.ts', import.meta.url))
const asyncUtils = loadModule(new URL('../src/utils/async.ts', import.meta.url))

test('Excel image policy defaults off and roundtrips without changing scheduling', async () => {
  for (const mode of ['off', 'auto_compact', 'warn']) {
    const initial = settings()
    const query = mountSettings(initial)
    try {
      await query.state.loadSettings()
      assert.equal(query.state.form.requestTuning.excelImageLimitPolicy, 'off')
      assert.equal(query.state.form.requestTuning.excelImageWarningRemaining, 8)
      assert.equal(query.state.form.requestTuning.excelImageCompactReserve, 3)
      query.state.form.requestTuning.excelImageLimitPolicy = mode
      await query.state.saveSettings()
      assert.equal(query.requests[0].requestTuning.excelImageLimitPolicy, mode)
      assert.equal(query.requests[0].rotationStrategy, initial.rotationStrategy)
    }
    finally { query.stop() }
  }
})

test('Excel warning thresholds reject unsafe combinations before saving', async () => {
  for (const [warning, reserve] of [[3, 3], [8, 0], [20, 3]]) {
    const warnings = []
    const query = mountSettings(settings(), message => warnings.push(message))
    try {
      await query.state.loadSettings()
      Object.assign(query.state.form.requestTuning, {
        excelImageLimitPolicy: 'warn',
        excelImageMaxCount: 20,
        excelImageWarningRemaining: warning,
        excelImageCompactReserve: reserve,
      })
      await query.state.saveSettings()
      assert.equal(query.requests.length, 0)
      assert.equal(warnings.length, 1)
    }
    finally { query.stop() }
  }
})

test('Excel image transport preserves explicit modes and inherited startup settings', async () => {
  for (const transport of [null, { mode: 'native' }, { mode: 'relay', publicUrl: 'https://images.example.com' }]) {
    const initial = settings()
    initial.requestTuning = { excelImageTransport: transport }
    const query = mountSettings(initial)
    try {
      await query.state.loadSettings()
      assert.deepEqual(JSON.parse(JSON.stringify(query.state.form.requestTuning.excelImageTransport)), transport)
      await query.state.saveSettings()
      assert.deepEqual(query.requests[0].requestTuning.excelImageTransport, transport)
      assert.equal(query.requests[0].rotationStrategy, initial.rotationStrategy)
    }
    finally { query.stop() }
  }
})

test('Excel relay requires a public HTTPS origin before settings can be saved', async () => {
  for (const publicUrl of ['', 'http://images.example.com', 'https://images.example.com/path', 'https://user@example.com', 'https://images.example.com?x=1', 'https://images.example.com#part', 'https://127.0.0.1', 'https://host.local']) {
    const warnings = []
    const query = mountSettings(settings(), message => warnings.push(message))
    try {
      await query.state.loadSettings()
      query.state.form.requestTuning.excelImageTransport = { mode: 'relay', publicUrl }
      await query.state.saveSettings()
      assert.equal(query.requests.length, 0, publicUrl)
      assert.equal(warnings.length, 1, publicUrl)
    }
    finally { query.stop() }
  }
})

test('Excel global defaults fill omission without replacing saved or explicitly empty models', async () => {
  for (const models of [undefined, [], ['custom-model'], ['gpt-5.6-sol', 'gpt-6-astra']]) {
    const initial = settings()
    if (models !== undefined)
      initial.excelDefaultModels = models
    const query = mountSettings(initial)
    try {
      await query.state.loadSettings()
      const expected = models ?? ['gpt-6-astra', 'gpt-5.6-sol', 'gpt-5.6-terra']
      assert.equal(query.state.form.excelDefaultModels, expected.join(', '))
      await query.state.saveSettings()
      assert.deepEqual(query.requests[0].excelDefaultModels, expected)
    }
    finally {
      query.stop()
    }
  }
})

test('affinity and binding TTL roundtrip without changing smart scheduling or reserved concurrency', async () => {
  const query = mountSettings({ ...settings(), openaiGuardianReservedConcurrency: 2 })
  try {
    await query.state.loadSettings()
    assert.equal(query.state.form.openaiAccountAffinity, 'strict')
    assert.equal(query.state.form.openaiSessionBindingTtlHours, 24)
    for (const mode of ['relaxed', 'strict']) {
      for (const hours of [1, 48, 720]) {
        query.state.form.openaiAccountAffinity = mode
        query.state.form.openaiSessionBindingTtlHours = hours
        await query.state.saveSettings()
        await query.state.loadSettings()
        const saved = query.requests.at(-1)
        assert.equal(saved.openaiAccountAffinity, mode)
        assert.equal(saved.openaiSessionBindingTtlHours, hours)
        assert.equal(query.state.form.openaiAccountAffinity, mode)
        assert.equal(query.state.form.openaiSessionBindingTtlHours, hours)
        assert.equal(saved.rotationStrategy, 'smart')
        assert.equal(saved.openaiGuardianReservedConcurrency, 2)
        assert.equal(Object.hasOwn(saved.requestTuning, 'openaiGuardianReservedConcurrency'), false)
      }
    }
  }
  finally { query.stop() }
})

test('invalid affinity or binding TTL never submits a settings update', async () => {
  const warnings = []
  const query = mountSettings(settings(), value => warnings.push(value))
  try {
    await query.state.loadSettings()
    query.state.form.openaiAccountAffinity = 'unknown'
    await query.state.saveSettings()
    query.state.form.openaiAccountAffinity = 'strict'
    for (const invalid of [0, 721, 1.5, Number.NaN, null]) {
      query.state.form.openaiSessionBindingTtlHours = invalid
      await query.state.saveSettings()
    }
    assert.equal(query.requests.length, 0)
    assert.equal(warnings.length, 6)
  }
  finally { query.stop() }
})

function mountSettings(initial, onWarning = assert.fail) {
  let saved = structuredClone(initial)
  const requests = []
  const notifications = { toast: { success: () => {}, warning: onWarning, error: assert.fail } }
  const asyncAction = loadModule(new URL('../src/composables/useAsyncAction.ts', import.meta.url), {
    vue,
    '@/api/request': apiErrors,
    '@/components/base/BaseToast': notifications,
    '@/utils/async': asyncUtils,
  })
  const dependencies = {
    './useExcelImageSettings': loadModule(new URL('../src/views/settings/composables/useExcelImageSettings.ts', import.meta.url), { vue }),
    '@/api/modules/settings': loadModule(new URL('../src/api/modules/settings.ts', import.meta.url), {
      '../request': () => { throw new Error('unexpected real API request') },
    }),
    '@/views/accounts/utils/schedulingForm': loadModule(new URL('../src/views/accounts/utils/schedulingForm.ts', import.meta.url)),
    vue,
    '@/api': {
      getSettings: async () => saved,
      updateSettings: async (payload) => {
        const data = structuredClone(payload)
        requests.push(data)
        saved = { ...data, updatedAt: initial.updatedAt }
        return saved
      },
    },
    '@/api/request': apiErrors,
    '@/components/base/BaseToast': notifications,
    '@/composables/useAsyncAction': asyncAction,
    '@/utils/async': asyncUtils,
  }
  const module = loadModule(new URL('../src/views/settings/composables/useSettingsForm.ts', import.meta.url), dependencies)
  const scope = vue.effectScope()
  const state = scope.run(() => module.useSettingsForm())
  return { state, requests, stop: () => scope.stop() }
}

function settings() {
  return {
    modelMappings: { 'client-model': 'upstream-model' },
    refreshMarginSeconds: 1800,
    refreshConcurrency: 4,
    maxConcurrentPerAccount: 5,
    requestIntervalMs: 25,
    rotationStrategy: 'smart',
    minCodexDesktopVersion: null,
    minCodexCliVersion: null,
    usageRetentionDays: 31,
    opsEventRetentionDays: 30,
    auditRetentionDays: 90,
    updatedAt: '2026-09-12T00:00:00Z',
  }
}

test('smart defaults and drafts are isolated; validation and save reload preserve configured values', async () => {
  const warnings = []
  const query = mountSettings(settings(), message => warnings.push(message))
  const other = mountSettings(settings())
  try {
    await query.state.loadSettings()
    const expected = { loadWeight: 1, quotaWeight: 0.8, healthWeight: 1, latencyWeight: 0.5, resetWeight: 0, queueWeight: 0, preferHigherWeight: false }
    assert.deepEqual(JSON.parse(JSON.stringify(query.state.form.requestTuning.smartScheduling)), expected)
    query.state.form.requestTuning.smartScheduling.loadWeight = 2.5
    query.state.form.requestTuning.smartScheduling.preferHigherWeight = true
    await query.state.saveSettings()
    await query.state.loadSettings()
    assert.equal(query.state.form.requestTuning.smartScheduling.loadWeight, 2.5)
    assert.equal(query.requests.at(-1).requestTuning.smartScheduling.preferHigherWeight, true)
    await other.state.loadSettings()
    assert.equal(other.state.form.requestTuning.smartScheduling.loadWeight, 1)
    for (const invalid of [-1, 10.1, 0.01, Number.NaN]) {
      query.state.form.requestTuning.smartScheduling.loadWeight = invalid
      await query.state.saveSettings()
    }
    for (const key of Object.keys(expected).filter(key => key !== 'preferHigherWeight'))
      query.state.form.requestTuning.smartScheduling[key] = 0
    await query.state.saveSettings()
    assert.equal(query.requests.length, 1)
    assert.equal(warnings.length, 5)
  }
  finally {
    query.stop()
    other.stop()
  }
})

test('global Excel defaults roundtrip exact models including explicit empty and reject invalid IDs', async () => {
  const warnings = []
  const query = mountSettings(settings(), message => warnings.push(message))
  try {
    await query.state.loadSettings()
    assert.equal(query.state.form.excelDefaultModels, 'gpt-6-astra, gpt-5.6-sol, gpt-5.6-terra')
    query.state.form.excelDefaultModels = 'gpt-6-astra, gpt-6-astra'
    await query.state.saveSettings()
    assert.deepEqual(query.requests.at(-1).excelDefaultModels, ['gpt-6-astra'])
    query.state.form.excelDefaultModels = 'invalid/model'
    await query.state.saveSettings()
    assert.equal(query.requests.length, 1)
    assert.equal(warnings.length, 1)
    query.state.form.excelDefaultModels = ''
    await query.state.saveSettings()
    assert.deepEqual(query.requests.at(-1).excelDefaultModels, [])
    await query.state.loadSettings()
    assert.equal(query.state.form.excelDefaultModels, '')
  }
  finally { query.stop() }
})

test('location remains opt-in and custom location round trips without changing other tuning', async () => {
  const query = mountSettings(settings())
  try {
    await query.state.loadSettings()
    assert.equal(query.state.form.requestTuning.openaiLocationOverrideEnabled, false)
    assert.equal(query.state.form.requestTuning.openaiRequestLocation, null)
    const location = { country: 'JP', region: 'Tokyo', city: 'Tokyo', timezone: 'Asia/Tokyo' }
    query.state.form.requestTuning.openaiRequestLocation = location
    query.state.form.requestTuning.openaiLocationOverrideEnabled = true
    await query.state.saveSettings()
    assert.deepEqual(query.requests.at(-1).requestTuning.openaiRequestLocation, location)
    query.state.form.requestTuning.openaiLocationOverrideEnabled = false
    await query.state.saveSettings()
    assert.deepEqual(query.requests.at(-1).requestTuning.openaiRequestLocation, location)
    assert.equal(query.requests.at(-1).requestTuning.accountBusyWaitEnabled, false)
    assert.equal(query.requests.at(-1).rotationStrategy, 'smart')
    query.state.form.requestTuning.openaiRequestLocation = null
    await query.state.saveSettings()
    assert.equal(query.requests.at(-1).requestTuning.openaiRequestLocation, null)
  }
  finally {
    query.stop()
  }
})

test('stream prefetch inherits defaults and round trips zero and exact custom bytes', async () => {
  for (const [overrides, defaults, expected] of [
    [{}, {}, 128 * 1024],
    [{ streamPrefetchBytes: null }, { streamPrefetchBytes: 256 * 1024 }, 256 * 1024],
    [{ streamPrefetchBytes: 0 }, { streamPrefetchBytes: 256 * 1024 }, 0],
  ]) {
    const query = mountSettings({ ...settings(), requestTuning: overrides, requestTuningDefaults: defaults })
    try {
      await query.state.loadSettings()
      assert.equal(query.state.form.requestTuning.streamPrefetchBytes, expected)
      for (const value of [expected, 0, 1, 512, 262144, 128 * 1024 * 1024, Number.MAX_SAFE_INTEGER]) {
        query.state.form.requestTuning.streamPrefetchBytes = value
        await query.state.saveSettings()
        assert.equal(query.requests.at(-1).requestTuning.streamPrefetchBytes, value)
        await query.state.loadSettings()
        assert.equal(query.state.form.requestTuning.streamPrefetchBytes, value)
      }
    }
    finally { query.stop() }
  }
})

test('stream prefetch rejects only invalid or inexact byte numbers before saving', async () => {
  const warnings = []
  const query = mountSettings(settings(), message => warnings.push(message))
  try {
    await query.state.loadSettings()
    for (const value of [-1, 0.5, Number.MAX_SAFE_INTEGER + 1, Number.NaN, Number.POSITIVE_INFINITY]) {
      query.state.form.requestTuning.streamPrefetchBytes = value
      await query.state.saveSettings()
    }
    assert.equal(query.requests.length, 0)
    assert.equal(warnings.length, 5)
    const component = readFileSync(new URL('../src/views/settings/components/RuntimeSettingsCard.vue', import.meta.url), 'utf8')
    assert.match(component, /tuningNumber\('streamPrefetchBytes', 1024\)/)
    assert.match(component, /v-model="tuningValues\.streamPrefetchKiB\.value"[^>]+step="any"/)
  }
  finally { query.stop() }
})

test('large request threshold inherits defaults and round trips zero and custom bytes', async () => {
  for (const [overrides, defaults, expected] of [
    [{}, {}, 15 * 1024 * 1024],
    [{ websocketLargeRequestThresholdBytes: null }, { websocketLargeRequestThresholdBytes: 8192 }, 8192],
    [{ websocketLargeRequestThresholdBytes: 0 }, { websocketLargeRequestThresholdBytes: 8192 }, 0],
  ]) {
    const query = mountSettings({ ...settings(), requestTuning: overrides, requestTuningDefaults: defaults })
    try {
      await query.state.loadSettings()
      assert.equal(query.state.form.requestTuning.websocketLargeRequestThresholdBytes, expected)
      for (const value of [expected, 0, 4096, 64 * 1024 * 1024]) {
        query.state.form.requestTuning.websocketLargeRequestThresholdBytes = value
        await query.state.saveSettings()
        assert.equal(query.requests.at(-1).requestTuning.websocketLargeRequestThresholdBytes, value)
        await query.state.loadSettings()
        assert.equal(query.state.form.requestTuning.websocketLargeRequestThresholdBytes, value)
      }
    }
    finally {
      query.stop()
    }
  }
})

test('large request threshold rejects invalid byte counts before saving', async () => {
  const warnings = []
  const query = mountSettings(settings(), message => warnings.push(message))
  try {
    await query.state.loadSettings()
    for (const value of [-1, 1.5, 64 * 1024 * 1024 + 1, Number.NaN, Number.POSITIVE_INFINITY]) {
      query.state.form.requestTuning.websocketLargeRequestThresholdBytes = value
      await query.state.saveSettings()
    }
    assert.equal(query.requests.length, 0)
    assert.equal(warnings.length, 5)
    const component = readFileSync(new URL('../src/views/settings/components/RuntimeSettingsCard.vue', import.meta.url), 'utf8')
    assert.match(component, /v-model="tuningValues\.websocketLargeRequestThresholdBytes\.value"/)
  }
  finally {
    query.stop()
  }
})

test('key queue bounds round trip and invalid values cannot be saved', async () => {
  for (const [maxWaitingPerKey, keyConcurrencyWaitTimeoutSeconds, valid] of [
    [0, 1, true],
    [1024, 600, true],
    [-1, 30, false],
    [1025, 30, false],
    [1.5, 30, false],
    [1, 0, false],
    [1, 601, false],
    [1, 2.5, false],
  ]) {
    const warnings = []
    const query = mountSettings(settings(), message => warnings.push(message))
    try {
      await query.state.loadSettings()
      assert.equal(query.state.form.requestTuning.maxWaitingPerKey, 0)
      assert.equal(query.state.form.requestTuning.keyConcurrencyWaitTimeoutSeconds, 30)
      Object.assign(query.state.form.requestTuning, { maxWaitingPerKey, keyConcurrencyWaitTimeoutSeconds })
      await query.state.saveSettings()
      assert.equal(query.requests.length, valid ? 1 : 0)
      assert.equal(warnings.length, valid ? 0 : 1)
      if (valid) {
        await query.state.loadSettings()
        assert.equal(query.state.form.requestTuning.maxWaitingPerKey, maxWaitingPerKey)
        assert.equal(query.state.form.requestTuning.keyConcurrencyWaitTimeoutSeconds, keyConcurrencyWaitTimeoutSeconds)
        assert.equal(query.state.form.rotationStrategy, 'smart')
      }
    }
    finally {
      query.stop()
    }
  }
})
test('runtime settings load and save without a global WS opening limit', async () => {
  for (const legacy of [false, true]) {
    const initial = {
      ...settings(),
      requestTuning: { maxRequestAttempts: 8, websocketHttpFallbackEnabled: false },
      requestTuningDefaults: { websocketMaxAgeMs: 60_000 },
    }
    if (legacy) {
      initial.requestTuning.websocketMaxConnecting = 4
      initial.requestTuningDefaults.websocketMaxConnecting = 8
    }
    const query = mountSettings(initial)
    try {
      assert.equal(Object.hasOwn(query.state.form.requestTuning, 'websocketMaxConnecting'), false)
      await query.state.loadSettings()
      assert.equal(query.state.form.requestTuning.maxRequestAttempts, 8)
      assert.equal(query.state.form.requestTuning.websocketHttpFallbackEnabled, false)
      assert.equal(query.state.form.requestTuning.websocketMaxAgeMs, 60_000)
      assert.equal(query.state.form.requestTuning.websocketMaxRetries, 5)
      assert.equal(Object.hasOwn(query.state.form.requestTuning, 'websocketMaxConnecting'), false)

      query.state.form.requestTuning.websocketMaxRetries = 2
      await query.state.saveSettings()
      assert.equal(query.requests.length, 1)
      assert.equal(Object.hasOwn(query.requests[0].requestTuning, 'websocketMaxConnecting'), false)
      assert.equal(query.requests[0].requestTuning.websocketMaxRetries, 2)
      assert.equal(query.requests[0].maxConcurrentPerAccount, 5)
      assert.deepEqual(query.requests[0].modelMappings, initial.modelMappings)

      await query.state.loadSettings()
      await query.state.saveSettings()
      assert.equal(query.requests.length, 2)
      assert.deepEqual(query.requests[1], query.requests[0])
    }
    finally {
      query.stop()
    }
  }
})

test('inherited runtime defaults never add the removed global WS opening limit', async () => {
  const query = mountSettings(settings())
  try {
    await query.state.loadSettings()
    await query.state.saveSettings()
    assert.equal(query.requests.length, 1)
    assert.deepEqual(query.requests[0].requestTuning, {
      smartScheduling: { loadWeight: 1, quotaWeight: 0.8, healthWeight: 1, latencyWeight: 0.5, resetWeight: 0, queueWeight: 0, preferHigherWeight: false },
      maxAccountSwitches: 31,
      maxRequestAttempts: 32,
      websocketMaxRetries: 5,
      websocketHttpFallbackEnabled: true,
      websocketLargeRequestThresholdBytes: 15 * 1024 * 1024,
      streamPrefetchBytes: 128 * 1024,
      websocketMaxAgeMs: 55 * 60 * 1_000,
      websocketStreamIdleTimeoutMs: 300_000,
      websocketFailureThreshold: 3,
      websocketFailureWindowMs: 30_000,
      websocketFailureOpenDurationMs: 30_000,
      rateLimitCooldownSeconds: 60,
      excelImageRelayBytes: 1024 * 1024 * 1024,
      excelImageRelayRequests: 128,
      excelImageRelayDownloads: 32,
      excelImageRelayEntries: 512,
      excelImageMaxBytes: 20 * 1024 * 1024,
      excelImageTotalBytes: 32 * 1024 * 1024,
      excelImageMaxCount: 20,
      excelImageLimitPolicy: 'off',
      excelImageWarningRemaining: 8,
      excelImageCompactReserve: 3,
      excelImageRelayTtlMinutes: 30,
      excelImageTransport: null,
      openaiLocationOverrideEnabled: false,
      openaiRequestLocation: null,
      maxWaitingPerKey: 0,
      keyConcurrencyWaitTimeoutSeconds: 30,
      accountBusyWaitEnabled: false,
      accountBusyWaitStickyMaxWaiting: 3,
      accountBusyWaitStickyTimeoutSeconds: 120,
      accountBusyWaitFallbackMaxWaiting: 100,
      accountBusyWaitFallbackTimeoutSeconds: 30,
    })
  }
  finally {
    query.stop()
  }
})

test('Excel image budgets roundtrip independently and reject invalid limits', async () => {
  const warnings = []
  const query = mountSettings(settings(), message => warnings.push(message))
  try {
    await query.state.loadSettings()
    for (const [field, value] of [
      ['excelImageRelayBytes', 64 * 1024 * 1024],
      ['excelImageRelayRequests', 8],
      ['excelImageRelayDownloads', 4],
      ['excelImageRelayEntries', 64],
      ['excelImageRelayTtlMinutes', 90],
      ['excelImageMaxBytes', 8 * 1024 * 1024],
      ['excelImageTotalBytes', 16 * 1024 * 1024],
      ['excelImageMaxCount', 32],
    ]) {
      query.state.form.requestTuning[field] = value
    }
    await query.state.saveSettings()
    await query.state.loadSettings()
    assert.equal(query.state.form.requestTuning.excelImageRelayBytes, 64 * 1024 * 1024)
    assert.equal(query.state.form.requestTuning.excelImageRelayDownloads, 4)
    assert.equal(query.state.form.requestTuning.excelImageRelayRequests, 8)
    assert.equal(query.state.form.requestTuning.excelImageRelayEntries, 64)
    assert.equal(query.state.form.requestTuning.excelImageRelayTtlMinutes, 90)
    assert.equal(query.state.form.requestTuning.excelImageMaxBytes, 8 * 1024 * 1024)
    assert.equal(query.state.form.requestTuning.excelImageTotalBytes, 16 * 1024 * 1024)
    assert.equal(query.state.form.requestTuning.excelImageMaxCount, 32)
    assert.equal(query.requests[0].requestTuning.rateLimitCooldownSeconds, 60)
    assert.equal(query.requests[0].maxConcurrentPerAccount, 5)
    for (const [field, invalid] of [
      ['excelImageRelayBytes', 1024],
      ['excelImageRelayBytes', 262144 * 1024 * 1024 + 1],
      ['excelImageRelayDownloads', 0],
      ['excelImageRelayDownloads', 129],
      ['excelImageRelayRequests', 0],
      ['excelImageRelayRequests', 513],
      ['excelImageRelayEntries', 0],
      ['excelImageRelayEntries', 1048577],
      ['excelImageRelayTtlMinutes', 0],
      ['excelImageRelayTtlMinutes', 10081],
      ['excelImageMaxBytes', 0],
      ['excelImageMaxBytes', 512 * 1024 * 1024 + 1],
      ['excelImageTotalBytes', 0],
      ['excelImageTotalBytes', 512 * 1024 * 1024 + 1],
      ['excelImageMaxCount', 0],
      ['excelImageMaxCount', 65537],
    ]) {
      const before = query.state.form.requestTuning[field]
      query.state.form.requestTuning[field] = invalid
      await query.state.saveSettings()
      assert.equal(query.requests.length, 1)
      query.state.form.requestTuning[field] = before
    }
    assert.equal(warnings.length, 16)
    for (const [field, invalid] of [
      ['excelImageRelayBytes', 1024 * 1024],
      ['excelImageRelayEntries', 16],
      ['excelImageTotalBytes', 4 * 1024 * 1024],
    ]) {
      const before = query.state.form.requestTuning[field]
      query.state.form.requestTuning[field] = invalid
      await query.state.saveSettings()
      assert.equal(query.requests.length, 1)
      query.state.form.requestTuning[field] = before
    }
    assert.equal(warnings.length, 19)
  }
  finally {
    query.stop()
  }
})

const busyWaitDefaults = {
  accountBusyWaitEnabled: false,
  accountBusyWaitStickyMaxWaiting: 3,
  accountBusyWaitStickyTimeoutSeconds: 120,
  accountBusyWaitFallbackMaxWaiting: 100,
  accountBusyWaitFallbackTimeoutSeconds: 30,
}

test('retired State settings do not enter the form or save payload', async () => {
  const query = mountSettings({
    ...settings(),
    turnStateProbeConcurrency: 10,
    turnStateProbeProxyId: 'proxy-test',
    turnStateInjectionEnabled: true,
  })
  try {
    await query.state.loadSettings()
    await query.state.saveSettings()
    for (const key of ['turnStateProbeConcurrency', 'turnStateProbeProxyId', 'turnStateInjectionEnabled']) {
      assert.equal(Object.hasOwn(query.state.form, key), false)
      assert.equal(Object.hasOwn(query.requests.at(-1), key), false)
    }
    assert.equal(query.requests.at(-1).maxConcurrentPerAccount, 5)
    assert.equal(query.requests.at(-1).rotationStrategy, 'smart')
  }
  finally {
    query.stop()
  }
})

test('location override defaults off and persists independently of scheduling', async () => {
  const query = mountSettings(settings())
  try {
    await query.state.loadSettings()
    assert.equal(query.state.form.requestTuning.openaiLocationOverrideEnabled, false)
    for (const enabled of [true, false, true]) {
      query.state.form.requestTuning.openaiLocationOverrideEnabled = enabled
      await query.state.saveSettings()
      await query.state.loadSettings()
      assert.equal(query.state.form.requestTuning.openaiLocationOverrideEnabled, enabled)
      assert.equal(query.requests.at(-1).rotationStrategy, 'smart')
      assert.deepEqual(busyWaitValues(query.requests.at(-1).requestTuning), busyWaitDefaults)
    }
  }
  finally {
    query.stop()
  }
})

function busyWaitValues(tuning) {
  return Object.fromEntries(Object.keys(busyWaitDefaults).map(key => [key, tuning[key]]))
}

test('account busy wait loads saved values and round trips edited settings', async () => {
  const initialWait = {
    accountBusyWaitEnabled: true,
    accountBusyWaitStickyMaxWaiting: 7,
    accountBusyWaitStickyTimeoutSeconds: 180,
    accountBusyWaitFallbackMaxWaiting: 150,
    accountBusyWaitFallbackTimeoutSeconds: 45,
  }
  const editedWait = {
    accountBusyWaitEnabled: true,
    accountBusyWaitStickyMaxWaiting: 9,
    accountBusyWaitStickyTimeoutSeconds: 240,
    accountBusyWaitFallbackMaxWaiting: 200,
    accountBusyWaitFallbackTimeoutSeconds: 60,
  }
  const query = mountSettings({
    ...settings(),
    requestTuning: { ...initialWait, websocketMaxRetries: 2 },
    requestTuningDefaults: busyWaitDefaults,
  })
  try {
    await query.state.loadSettings()
    assert.deepEqual(busyWaitValues(query.state.form.requestTuning), initialWait)
    Object.assign(query.state.form.requestTuning, editedWait)
    await query.state.saveSettings()
    assert.equal(query.requests.length, 1)
    assert.deepEqual(busyWaitValues(query.requests[0].requestTuning), editedWait)
    assert.equal(query.requests[0].requestTuning.websocketMaxRetries, 2)
    assert.equal(query.requests[0].maxConcurrentPerAccount, 5)
    assert.equal(query.requests[0].rotationStrategy, 'smart')

    Object.assign(query.state.form.requestTuning, busyWaitDefaults)
    await query.state.loadSettings()
    assert.deepEqual(busyWaitValues(query.state.form.requestTuning), editedWait)
    await query.state.saveSettings()
    assert.equal(query.requests.length, 2)
    assert.deepEqual(query.requests[1], query.requests[0])
  }
  finally {
    query.stop()
  }
})

test('account busy wait defaults inherit per field and preserve an explicit false', async () => {
  const serverDefaults = {
    accountBusyWaitEnabled: true,
    accountBusyWaitStickyMaxWaiting: 5,
    accountBusyWaitStickyTimeoutSeconds: 150,
    accountBusyWaitFallbackMaxWaiting: 125,
    accountBusyWaitFallbackTimeoutSeconds: 40,
  }
  for (const { defaults, tuning, expected } of [
    { defaults: undefined, tuning: undefined, expected: busyWaitDefaults },
    { defaults: serverDefaults, tuning: undefined, expected: serverDefaults },
    {
      defaults: serverDefaults,
      tuning: { accountBusyWaitEnabled: false, accountBusyWaitStickyMaxWaiting: 8 },
      expected: { ...serverDefaults, accountBusyWaitEnabled: false, accountBusyWaitStickyMaxWaiting: 8 },
    },
    {
      defaults: { accountBusyWaitStickyTimeoutSeconds: 150 },
      tuning: { accountBusyWaitEnabled: true, accountBusyWaitFallbackMaxWaiting: 250 },
      expected: {
        ...busyWaitDefaults,
        accountBusyWaitEnabled: true,
        accountBusyWaitStickyTimeoutSeconds: 150,
        accountBusyWaitFallbackMaxWaiting: 250,
      },
    },
  ]) {
    const query = mountSettings({
      ...settings(),
      requestTuning: tuning,
      requestTuningDefaults: defaults,
    })
    try {
      assert.deepEqual(busyWaitValues(query.state.form.requestTuning), busyWaitDefaults)
      await query.state.loadSettings()
      assert.deepEqual(busyWaitValues(query.state.form.requestTuning), expected)
      await query.state.saveSettings()
      assert.equal(query.requests.length, 1)
      assert.deepEqual(busyWaitValues(query.requests[0].requestTuning), expected)
    }
    finally {
      query.stop()
    }
  }
})

test('disabling account busy wait preserves all numeric settings across save and reload', async () => {
  const initialWait = {
    accountBusyWaitEnabled: true,
    accountBusyWaitStickyMaxWaiting: 11,
    accountBusyWaitStickyTimeoutSeconds: 210,
    accountBusyWaitFallbackMaxWaiting: 230,
    accountBusyWaitFallbackTimeoutSeconds: 55,
  }
  const query = mountSettings({ ...settings(), requestTuning: initialWait })
  try {
    await query.state.loadSettings()
    query.state.form.requestTuning.accountBusyWaitEnabled = false
    await query.state.saveSettings()
    assert.equal(query.requests.length, 1)
    const disabledWait = { ...initialWait, accountBusyWaitEnabled: false }
    assert.deepEqual(busyWaitValues(query.requests[0].requestTuning), disabledWait)
    Object.assign(query.state.form.requestTuning, busyWaitDefaults)
    await query.state.loadSettings()
    assert.deepEqual(busyWaitValues(query.state.form.requestTuning), disabledWait)

    query.state.form.requestTuning.accountBusyWaitEnabled = true
    await query.state.saveSettings()
    assert.equal(query.requests.length, 2)
    assert.deepEqual(busyWaitValues(query.requests[1].requestTuning), initialWait)
  }
  finally {
    query.stop()
  }
})

test('account busy wait accepts inclusive numeric bounds when enabled or disabled', async () => {
  for (const enabled of [false, true]) {
    for (const [maxWaiting, timeoutSeconds] of [[1, 1], [1000, 600]]) {
      const expected = {
        accountBusyWaitEnabled: enabled,
        accountBusyWaitStickyMaxWaiting: maxWaiting,
        accountBusyWaitStickyTimeoutSeconds: timeoutSeconds,
        accountBusyWaitFallbackMaxWaiting: maxWaiting,
        accountBusyWaitFallbackTimeoutSeconds: timeoutSeconds,
      }
      const query = mountSettings(settings())
      try {
        await query.state.loadSettings()
        Object.assign(query.state.form.requestTuning, expected)
        await query.state.saveSettings()
        assert.equal(query.requests.length, 1)
        assert.deepEqual(busyWaitValues(query.requests[0].requestTuning), expected)
      }
      finally {
        query.stop()
      }
    }
  }
})

for (const [field, upperBound] of [
  ['accountBusyWaitStickyMaxWaiting', 1000],
  ['accountBusyWaitStickyTimeoutSeconds', 600],
  ['accountBusyWaitFallbackMaxWaiting', 1000],
  ['accountBusyWaitFallbackTimeoutSeconds', 600],
]) {
  for (const enabled of [false, true]) {
    test(`account busy wait rejects invalid ${field} without saving when enabled=${enabled}`, async () => {
      const warnings = []
      const query = mountSettings(settings(), message => warnings.push(message))
      try {
        await query.state.loadSettings()
        query.state.form.requestTuning.accountBusyWaitEnabled = enabled
        const validValue = query.state.form.requestTuning[field]
        const invalidValues = [0, -1, 1.5, upperBound + 1, Number.NaN, Infinity, -Infinity, '', '3', null, undefined]
        for (const value of invalidValues) {
          query.state.form.requestTuning[field] = value
          await query.state.saveSettings()
          assert.equal(query.requests.length, 0, `${field}=${String(value)} must not reach updateSettings`)
          assert.equal(query.state.saving.value, false)
        }
        assert.deepEqual(warnings, invalidValues.map(() => 'OpenAI 账号忙时等待人数须为 1–1000 的整数，等待秒数须为 1–600 的整数'))

        query.state.form.requestTuning[field] = validValue
        await query.state.saveSettings()
        assert.equal(query.requests.length, 1, 'correcting the value must allow saving again')
        assert.equal(query.requests[0].requestTuning[field], validValue)
        assert.equal(query.requests[0].requestTuning.accountBusyWaitEnabled, enabled)
        assert.equal(warnings.length, invalidValues.length)
      }
      finally {
        query.stop()
      }
    })
  }
}

test('settings controls and API type no longer expose the global WS opening limit', () => {
  const component = readFileSync(new URL('../src/views/settings/components/RuntimeSettingsCard.vue', import.meta.url), 'utf8')
  const api = readFileSync(new URL('../src/api/modules/settings.ts', import.meta.url), 'utf8')
  assert.doesNotMatch(component, /websocketMaxConnecting/)
  assert.doesNotMatch(api, /websocketMaxConnecting/)
  assert.match(component, /v-model="maxConcurrentPerAccount"/)
  assert.match(component, /v-model="requestTuning\.websocketHttpFallbackEnabled"/)
})

test('Excel controls are isolated and shared retries remain in the common configuration', () => {
  const common = readFileSync(new URL('../src/views/settings/components/RuntimeSettingsCard.vue', import.meta.url), 'utf8')
  const excel = readFileSync(new URL('../src/views/settings/components/ExcelSettingsCard.vue', import.meta.url), 'utf8')
  const page = readFileSync(new URL('../src/views/settings/index.vue', import.meta.url), 'utf8')
  assert.doesNotMatch(common, /excelImage|imageRelayUrl|imageMode/)
  for (const key of ['websocketMaxRetries', 'maxAccountSwitches', 'maxRequestAttempts', 'rateLimitCooldownSeconds']) {
    assert.match(common, new RegExp(`v-model="tuningValues\\.${key}\\.value"`))
    assert.doesNotMatch(excel, new RegExp(key))
  }
  assert.match(page, /v-show="configurationScope === 'common'"/)
  assert.match(page, /v-show="configurationScope === 'excel'"/)
  assert.equal((page.match(/v-model:models="form\.excelDefaultModels"/g) ?? []).length, 1)
  assert.match(page, /保存全部设置/)
  assert.match(excel, /链接有效期不是模型记忆图片的时长/)
})

test('Excel MiB display roundtrips exact legacy byte values without rounding', async () => {
  for (const bytes of [1, 1024 * 1024 - 1, 20 * 1024 * 1024 + 1, 128 * 1024 * 1024]) {
    const initial = settings()
    initial.requestTuning = { excelImageMaxBytes: bytes, excelImageTotalBytes: 128 * 1024 * 1024 }
    const query = mountSettings(initial)
    try {
      await query.state.loadSettings()
      const field = query.state.excelImages.fields.find(field => field.key === 'excelImageMaxBytes')
      assert.equal(Number(field.input.value) * 1024 * 1024, bytes)
      const displayed = field.input.value
      field.input.value = displayed
      assert.equal(field.error.value, '')
      await query.state.saveSettings()
      await query.state.loadSettings()
      assert.equal(query.requests[0].requestTuning.excelImageMaxBytes, bytes)
      assert.equal(Number(field.input.value) * 1024 * 1024, bytes)
    }
    finally { query.stop() }
  }
})

test('editing Excel MiB changes only the intended byte field, not shared or native settings', async () => {
  const initial = settings()
  initial.requestTuning = {
    websocketMaxRetries: 7,
    maxAccountSwitches: 4,
    maxRequestAttempts: 9,
    rateLimitCooldownSeconds: 37,
    websocketHttpFallbackEnabled: false,
    websocketLargeRequestThresholdBytes: 1234567,
    websocketMaxAgeMs: 123456,
    openaiLocationOverrideEnabled: false,
    excelImageTransport: null,
  }
  const query = mountSettings(initial)
  try {
    await query.state.loadSettings()
    const before = JSON.parse(JSON.stringify(query.state.form.requestTuning))
    const field = query.state.excelImages.fields.find(field => field.key === 'excelImageMaxBytes')
    field.input.value = '20.5'
    assert.equal(query.state.form.requestTuning.excelImageMaxBytes, 21495808)
    await query.state.saveSettings()
    const after = query.requests[0].requestTuning
    assert.deepEqual(after, { ...before, excelImageMaxBytes: 21495808 })
    for (const key of ['modelMappings', 'rotationStrategy', 'refreshMarginSeconds', 'refreshConcurrency', 'maxConcurrentPerAccount', 'requestIntervalMs'])
      assert.deepEqual(query.requests[0][key], initial[key])
    await query.state.saveSettings()
    assert.deepEqual(query.requests[1], query.requests[0])
  }
  finally { query.stop() }
})

test('invalid Excel display drafts block stale-value saves and recover after correction', async () => {
  const warnings = []
  const query = mountSettings(settings(), message => warnings.push(message))
  try {
    await query.state.loadSettings()
    const field = query.state.excelImages.fields.find(field => field.key === 'excelImageMaxBytes')
    const initial = query.state.form.requestTuning.excelImageMaxBytes
    const invalid = ['', ' ', '0', '-1', '513', 'Infinity', 'NaN', '0.00000001']
    for (const raw of invalid) {
      field.input.value = raw
      assert.ok(field.error.value, raw)
      await query.state.saveSettings()
      assert.equal(query.requests.length, 0)
      assert.equal(query.state.form.requestTuning.excelImageMaxBytes, initial)
    }
    assert.equal(warnings.length, invalid.length)
    field.input.value = '8'
    assert.equal(field.error.value, '')
    await query.state.saveSettings()
    assert.equal(query.requests[0].requestTuning.excelImageMaxBytes, 8 * 1024 * 1024)
  }
  finally { query.stop() }
})

test('Excel reference maxima save and reload without changing shared retry controls', async () => {
  const query = mountSettings(settings())
  try {
    await query.state.loadSettings()
    const before = query.state.form.requestTuning.websocketMaxRetries
    const limits = {
      excelImageMaxBytes: 512 * 1024 * 1024,
      excelImageTotalBytes: 512 * 1024 * 1024,
      excelImageMaxCount: 65536,
      excelImageRelayBytes: 262144 * 1024 * 1024,
      excelImageRelayEntries: 1048576,
      excelImageRelayTtlMinutes: 10080,
    }
    Object.assign(query.state.form.requestTuning, limits)
    await query.state.saveSettings()
    assert.equal(query.requests.length, 1)
    for (const [key, maximum] of Object.entries(limits)) {
      assert.equal(query.requests[0].requestTuning[key], maximum)
      assert.equal(query.state.form.requestTuning[key], maximum)
    }
    assert.equal(query.requests[0].requestTuning.websocketMaxRetries, before)
  }
  finally { query.stop() }
})

test('Excel image mode toggling preserves draft relay address and does not rewrite limits', async () => {
  const query = mountSettings(settings())
  try {
    await query.state.loadSettings()
    const images = query.state.excelImages
    const before = JSON.parse(JSON.stringify(query.state.form.requestTuning))
    images.mode.value = 'relay'
    images.relayUrl.value = 'https://synthetic-images.example.com'
    for (const mode of ['native', 'inherit', 'native']) {
      images.mode.value = mode
      assert.equal(images.mode.value, mode)
      images.mode.value = 'relay'
      assert.equal(images.relayUrl.value, 'https://synthetic-images.example.com')
    }
    images.mode.value = 'inherit'
    assert.deepEqual(JSON.parse(JSON.stringify(query.state.form.requestTuning)), before)
    await query.state.saveSettings()
    assert.equal(query.requests[0].requestTuning.excelImageTransport, null)
    const count = images.fields.find(field => field.key === 'excelImageMaxCount')
    count.input.value = ''
    assert.ok(images.errors.value.length)
    await query.state.loadSettings()
    assert.equal(count.input.value, '20')
    assert.equal(images.errors.value.length, 0)
  }
  finally { query.stop() }
})
