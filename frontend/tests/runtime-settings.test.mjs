/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'
import { createTestRequire as createRequire } from './helpers/i18n.mjs'

const require = createRequire(import.meta.url)

function loadModule(filename, dependencies = {}) {
  const { outputText } = ts.transpileModule(readFileSync(filename, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  const exports = {}
  runInNewContext(outputText, {
    URL,
    TextEncoder,
    structuredClone,
    exports,
    require: name => dependencies[name] ?? (name === '@/components/request-profile/changes' ? loadModule(new URL('../src/components/request-profile/changes.ts', import.meta.url)) : name === '../utils/warmup-schedule' ? loadModule(new URL('../src/views/settings/utils/warmup-schedule.ts', import.meta.url)) : name === '../components/error-protection/form' ? loadModule(new URL('../src/views/settings/components/error-protection/form.ts', import.meta.url)) : require(name)),
  }, { filename: String(filename) })
  return exports
}

const apiErrors = loadModule(new URL('../src/api/error.ts', import.meta.url))
const asyncUtils = loadModule(new URL('../src/utils/async.ts', import.meta.url))

test('warmup rules retain legacy schedules and roundtrip independent hour/minute pairs', async () => {
  const warnings = []
  const query = mountSettings({ ...settings(), accountWarmupEnabled: true, accountWarmupCron: '30 1 * * *', accountWarmupModel: 'fixture-model' }, message => warnings.push(message))
  try {
    await query.state.loadSettings()
    assert.deepEqual(Array.from(query.state.form.accountWarmupCron), ['30 1 * * *'])
    query.state.form.accountWarmupCron.push('45 8 * * *', '0 0 * * MON,FRI')
    await query.state.saveSettings()
    assert.equal(query.requests.at(-1).accountWarmupCron, '30 1 * * *;45 8 * * *;0 0 * * MON,FRI')
    await query.state.loadSettings()
    assert.deepEqual(Array.from(query.state.form.accountWarmupCron), ['30 1 * * *', '45 8 * * *', '0 0 * * MON,FRI'])
    const count = query.requests.length
    for (const rules of [[], ['0 8 * *'], ['0 8 * * *;'], ['0 8 * * *\n0 9 * * *'], Array.from({ length: 26 }).fill('0 8 * * *')]) {
      query.state.form.accountWarmupCron = rules
      await query.state.saveSettings()
      assert.equal(query.requests.length, count)
    }
    assert.equal(warnings.length, 5)
    query.state.form.accountWarmupEnabled = false
    query.state.form.accountWarmupCron = []
    await query.state.saveSettings()
    assert.equal(query.requests.at(-1).accountWarmupCron, '')
    await query.state.loadSettings()
    assert.deepEqual(Array.from(query.state.form.accountWarmupCron), [])
  }
  finally { query.stop() }
})

test('Retry-After limit defaults to 240 and saves zero and custom values without changing queue settings', async () => {
  const warnings = []
  const query = mountSettings(settings(), message => warnings.push(message))
  try {
    await query.state.loadSettings()
    assert.equal(query.state.form.requestTuning.maxRetryAfterSeconds, 240)
    const waiting = query.state.form.requestTuning.accountQueueTimeoutSeconds
    for (const seconds of [0, 400, 4_294_967_295]) {
      query.state.form.requestTuning.maxRetryAfterSeconds = seconds
      await query.state.saveSettings()
      assert.equal(query.requests.at(-1).requestTuning.maxRetryAfterSeconds, seconds)
      assert.equal(query.requests.at(-1).requestTuning.accountQueueTimeoutSeconds, waiting)
    }
    const count = query.requests.length
    for (const seconds of [-1, 0.5, Number.NaN, 4_294_967_296]) {
      query.state.form.requestTuning.maxRetryAfterSeconds = seconds
      await query.state.saveSettings()
      assert.equal(query.requests.length, count)
    }
    assert.equal(warnings.length, 4)
  }
  finally { query.stop() }
})

test('runtime display units preserve saved bytes and milliseconds without a global Fast field', async () => {
  const initial = settings()
  initial.requestIntervalMs = 50
  initial.responsesMaxDecompressedBodyBytes = 64 * 1024 * 1024 + 1
  initial.requestTuning = { ...initial.requestTuning, streamPrefetchBytes: 513 }
  initial.disableFast = true
  const query = mountSettings(initial)
  try {
    await query.state.loadSettings()
    const units = query.state.unitFields
    assert.equal(units.intervalSeconds.input.value, '0.05')
    assert.equal(units.bodyMiB.input.value, String(initial.responsesMaxDecompressedBodyBytes / 1048576))
    assert.equal(query.state.form.openaiWebsocketPool.maxAgeMs, 3300000)
    await query.state.saveSettings()
    assert.equal(query.requests.at(-1).responsesMaxDecompressedBodyBytes, initial.responsesMaxDecompressedBodyBytes)
    assert.equal(query.requests.at(-1).requestIntervalMs, 50)
    assert.equal(Object.hasOwn(query.requests.at(-1), 'disableFast'), false)
    units.intervalSeconds.input.value = '1.001'
    units.bodyMiB.input.value = '1.5'
    assert.equal(Object.hasOwn(units, 'prefetchKiB'), false)
    query.state.form.openaiWebsocketPool.streamIdleTimeoutMs = 10001
    await query.state.saveSettings()
    const saved = query.requests.at(-1)
    assert.equal(saved.requestIntervalMs, 1001)
    assert.equal(saved.responsesMaxDecompressedBodyBytes, 1572864)
    assert.equal(saved.requestTuning.websocketLargeRequestThresholdBytes, undefined)
    assert.equal(saved.requestTuning.streamPrefetchBytes, 513)
    assert.equal(saved.openaiWebsocketPool.streamIdleTimeoutMs, 10001)
  }
  finally { query.stop() }
})

test('unit drafts reject empty and sub-byte or sub-millisecond inputs instead of saving stale values', async () => {
  const warnings = []
  const query = mountSettings(settings(), warning => warnings.push(warning))
  try {
    await query.state.loadSettings()
    for (const [key, raw] of [['bodyMiB', ''], ['intervalSeconds', '0.0001'], ['failureWindowSeconds', 'NaN'], ['bodyMiB', '0.000000001']]) {
      query.state.unitFields[key].input.value = raw
      await query.state.saveSettings()
      assert.equal(query.requests.length, 0)
      assert.equal(query.state.hasChanges.value, true)
      await query.state.loadSettings()
      assert.equal(query.state.unitFields[key].invalid.value, false)
    }
    assert.equal(warnings.length, 4)
  }
  finally { query.stop() }
})

test('affinity and binding TTL roundtrip without changing smart scheduling or reserved concurrency', async () => {
  const query = mountSettings({ ...settings(), openaiGuardianReservedConcurrency: 2 })
  try {
    await query.state.loadSettings()
    assert.equal(query.state.form.accountAffinity, 'strict')
    assert.equal(query.state.form.sessionAffinityTtlHours, 24)
    for (const mode of ['relaxed', 'strict', 'preferred']) {
      for (const hours of [1, 48, 720]) {
        query.state.form.accountAffinity = mode
        query.state.form.sessionAffinityTtlHours = hours
        await query.state.saveSettings()
        await query.state.loadSettings()
        const saved = query.requests.at(-1)
        assert.equal(saved.accountAffinity, mode)
        assert.equal(saved.sessionAffinityTtlHours, hours)
        assert.equal(query.state.form.accountAffinity, mode)
        assert.equal(query.state.form.sessionAffinityTtlHours, hours)
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
    query.state.form.accountAffinity = 'unknown'
    await query.state.saveSettings()
    query.state.form.accountAffinity = 'strict'
    for (const invalid of [0, 721, 1.5, Number.NaN, null]) {
      query.state.form.sessionAffinityTtlHours = invalid
      await query.state.saveSettings()
    }
    assert.equal(query.requests.length, 0)
    assert.equal(warnings.length, 6)
  }
  finally { query.stop() }
})

test('Codex quota overdraft defaults off and saves the exact probe model', async () => {
  const query = mountSettings(settings())
  try {
    await query.state.loadSettings()
    assert.equal(query.state.form.codexQuotaOverdraftEnabled, false)
    assert.equal(query.state.form.codexQuotaOverdraftProbeModel, 'gpt-6-astra')
    query.state.form.codexQuotaOverdraftEnabled = true
    query.state.form.codexQuotaOverdraftProbeModel = '  gpt-6-astra-preview  '
    await query.state.saveSettings()
    assert.equal(query.requests.at(-1).codexQuotaOverdraftEnabled, true)
    assert.equal(query.requests.at(-1).codexQuotaOverdraftProbeModel, 'gpt-6-astra-preview')
  }
  finally { query.stop() }
})

test('Codex quota overdraft rejects empty enabled models, control characters and oversized names', async () => {
  const warnings = []
  const query = mountSettings(settings(), message => warnings.push(message))
  try {
    await query.state.loadSettings()
    query.state.form.codexQuotaOverdraftEnabled = true
    for (const invalid of ['', `gpt-6-astra${String.fromCharCode(10)}other`, 'x'.repeat(129)]) {
      query.state.form.codexQuotaOverdraftProbeModel = invalid
      await query.state.saveSettings()
    }
    assert.equal(query.requests.length, 0)
    assert.equal(warnings.length, 3)
  }
  finally { query.stop() }
})

function mountSettings(initial, onWarning = assert.fail, apiOverrides = {}) {
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
    '@/api/modules/proxies': loadModule(new URL('../src/api/modules/proxies.ts', import.meta.url), {
      '../request': () => { throw new Error('unexpected real API request') },
    }),
    '@/utils/location': loadModule(new URL('../src/utils/location.ts', import.meta.url)),
    './useRuntimeUnits': loadModule(new URL('../src/views/settings/composables/useRuntimeUnits.ts', import.meta.url), { vue }),
    '@/api/modules/settings': loadModule(new URL('../src/api/modules/settings.ts', import.meta.url), {
      '../request': () => { throw new Error('unexpected real API request') },
    }),
    '@/views/accounts/utils/schedulingForm': loadModule(new URL('../src/views/accounts/utils/schedulingForm.ts', import.meta.url)),
    vue,
    '@/api': {
      getSettings: async () => saved,
      updateSettings: async (payload) => {
        function reactivePath(value, path = 'payload') {
          if (vue.isProxy(value))
            return path
          if (value && typeof value === 'object') {
            for (const [key, child] of Object.entries(value)) {
              const found = reactivePath(child, `${path}.${key}`)
              if (found)
                return found
            }
          }
        }
        assert.equal(reactivePath(payload), undefined, 'settings submission must be detached from reactive drafts')
        const data = structuredClone(payload)
        requests.push(data)
        saved = { ...data, configRevision: saved.configRevision + 1, updatedAt: initial.updatedAt }
        return saved
      },
      ...apiOverrides,
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
    openaiRequestSettings: { requestCountry: { mode: 'follow_location' }, oauthGuardianReviewModel: true, residency: 'us', accountRoutingOverride: null },
    openaiWebsocketPool: { enabled: true, maxAgeMs: 3300000, fastPathBudgetMs: 800, idleTimeoutMs: 300000, streamIdleTimeoutMs: 300000 },
    configRevision: 7,
    modelMappings: { 'client-model': 'upstream-model' },
    refreshMarginSeconds: 1800,
    refreshConcurrency: 4,
    maxConcurrentPerAccount: 5,
    requestIntervalMs: 25,
    rotationStrategy: 'smart',
    quotaResetWindowWeights: { fiveHour: 0, weekly: 0, monthly: 0 },
    quotaResetWindowWeightsDefaults: { fiveHour: 0, weekly: 0, monthly: 0 },
    minCodexDesktopVersion: null,
    minCodexCliVersion: null,
    usageRetentionDays: 31,
    opsEventRetentionDays: 30,
    auditRetentionDays: 90,
    updatedAt: '2026-09-12T00:00:00Z',
  }
}

test('settings saves use the loaded revision and advance only after successful saves', async () => {
  const query = mountSettings(settings())
  try {
    await query.state.loadSettings()
    await query.state.saveSettings()
    assert.equal(query.requests[0].configRevision, 7)
    await query.state.saveSettings()
    assert.equal(query.requests[1].configRevision, 8)
  }
  finally { query.stop() }
})

test('settings conflict preserves the draft and never retries using a newer revision', async () => {
  let reads = 0
  const writes = []
  const query = mountSettings(settings(), assert.fail, {
    getSettings: async () => {
      reads += 1
      return { ...settings(), configRevision: reads === 1 ? 7 : 8 }
    },
    updateSettings: async (payload) => {
      writes.push(payload)
      throw new apiErrors.ApiError('Settings changed; reload before saving.', 409, 40901)
    },
  })
  try {
    await query.state.loadSettings()
    query.state.form.requestIntervalMs = 123
    await query.state.saveSettings()
    await vue.nextTick()
    assert.equal(reads, 1, 'conflict must not reload and discard unsaved edits')
    assert.equal(writes.length, 1, 'conflict must not automatically retry')
    assert.equal(query.state.form.requestIntervalMs, 123)
    assert.equal(query.state.hasChanges.value, true)
    await query.state.saveSettings()
    assert.equal(writes[1].configRevision, 7, 'another click remains stale until explicit reload')
    await query.state.loadSettings()
    await query.state.saveSettings()
    assert.equal(writes[2].configRevision, 8)
  }
  finally { query.stop() }
})

test('smart defaults and drafts are isolated; validation and save reload preserve configured values', async () => {
  const warnings = []
  const query = mountSettings(settings(), message => warnings.push(message))
  const other = mountSettings(settings())
  try {
    await query.state.loadSettings()
    const expected = { loadWeight: 1, quotaWeight: 0.8, healthWeight: 1, latencyWeight: 0.5, resetWeight: 0, queueWeight: 0, preferHigherWeight: false, resetWindowWeights: { fiveHour: 0, weekly: 0, monthly: 0 } }
    assert.deepEqual(JSON.parse(JSON.stringify(query.state.form.requestTuning.smartScheduling)), expected)
    query.state.form.requestTuning.smartScheduling.loadWeight = 2.5
    query.state.form.requestTuning.smartScheduling.preferHigherWeight = true
    query.state.form.requestTuning.smartScheduling.resetWindowWeights = { fiveHour: 1.5, weekly: 2, monthly: 3 }
    await query.state.saveSettings()
    await query.state.loadSettings()
    assert.equal(query.state.form.requestTuning.smartScheduling.loadWeight, 2.5)
    assert.equal(query.requests.at(-1).requestTuning.smartScheduling.preferHigherWeight, true)
    assert.deepEqual(JSON.parse(JSON.stringify(query.state.form.requestTuning.smartScheduling.resetWindowWeights)), { fiveHour: 1.5, weekly: 2, monthly: 3 })
    await other.state.loadSettings()
    assert.equal(other.state.form.requestTuning.smartScheduling.loadWeight, 1)
    assert.deepEqual(JSON.parse(JSON.stringify(other.state.form.requestTuning.smartScheduling.resetWindowWeights)), { fiveHour: 0, weekly: 0, monthly: 0 })
    for (const invalid of [-1, 10.1, 0.01, Number.NaN]) {
      query.state.form.requestTuning.smartScheduling.loadWeight = invalid
      await query.state.saveSettings()
    }
    for (const key of Object.keys(expected).filter(key => key !== 'preferHigherWeight' && key !== 'resetWindowWeights'))
      query.state.form.requestTuning.smartScheduling[key] = 0
    await query.state.saveSettings()
    assert.equal(query.requests.length, 1)
    assert.equal(warnings.length, 5)
    query.state.form.requestTuning.smartScheduling.loadWeight = 1
    for (const key of ['fiveHour', 'weekly', 'monthly']) {
      for (const invalid of [-1, 10.1, 0.01, Number.NaN]) {
        query.state.form.requestTuning.smartScheduling.resetWindowWeights[key] = invalid
        await query.state.saveSettings()
      }
      query.state.form.requestTuning.smartScheduling.resetWindowWeights[key] = 0
    }
    assert.equal(query.requests.length, 1)
    assert.equal(warnings.length, 17)
    assert.deepEqual(query.requests[0].requestTuning.smartScheduling.resetWindowWeights, { fiveHour: 1.5, weekly: 2, monthly: 3 })
  }
  finally {
    query.stop()
    other.stop()
  }
})

test('location remains opt-in and custom location round trips without changing other tuning', async () => {
  const query = mountSettings(settings())
  try {
    await query.state.loadSettings()
    assert.equal(query.state.form.requestTuning.openaiLocationOverrideEnabled, false)
    assert.equal(query.state.form.requestTuning.openaiRequestLocation.city, 'Piketon')
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
    assert.deepEqual(query.requests.at(-1).requestTuning.openaiRequestLocation, location)
  }
  finally {
    query.stop()
  }
})

test('location normalizes enabled input and ignores incomplete disabled drafts', async () => {
  const warnings = []
  const query = mountSettings(settings(), message => warnings.push(message))
  try {
    await query.state.loadSettings()
    query.state.form.requestTuning.openaiLocationOverrideEnabled = true
    query.state.form.requestTuning.openaiRequestLocation = {
      country: ' jp ',
      region: ' Tokyo ',
      city: ' Tokyo ',
      timezone: ' Asia/Tokyo ',
    }
    await query.state.saveSettings()
    const saved = { country: 'JP', region: 'Tokyo', city: 'Tokyo', timezone: 'Asia/Tokyo' }
    assert.deepEqual(query.requests.at(-1).requestTuning.openaiRequestLocation, saved)
    query.state.form.requestTuning.openaiRequestLocation.city = ''
    await query.state.saveSettings()
    assert.equal(warnings.length, 1)
    assert.equal(query.requests.length, 1)
    query.state.form.requestTuning.openaiLocationOverrideEnabled = false
    await query.state.saveSettings()
    assert.deepEqual(query.requests.at(-1).requestTuning.openaiRequestLocation, saved)
    await query.state.loadSettings()
    query.state.form.requestTuning.openaiLocationOverrideEnabled = true
    assert.deepEqual({ ...query.state.form.requestTuning.openaiRequestLocation }, saved)
  }
  finally { query.stop() }
})

test('retired size threshold is omitted when loading and saving legacy settings', async () => {
  const query = mountSettings({ ...settings(), requestTuning: { websocketLargeRequestThresholdBytes: 4096 } })
  try {
    await query.state.loadSettings()
    assert.equal(query.state.form.requestTuning.websocketLargeRequestThresholdBytes, undefined)
    await query.state.saveSettings()
    assert.equal(query.requests.at(-1).requestTuning.websocketLargeRequestThresholdBytes, undefined)
  }
  finally { query.stop() }
})

test('key queue bounds round trip and invalid values cannot be saved', async () => {
  for (const [maxWaitingPerKey, keyQueueTimeoutSeconds, valid] of [
    [0, 1, true],
    [1024, 120, true],
    [-1, 30, false],
    [1025, 30, false],
    [1.5, 30, false],
    [1, 0, false],
    [1, 121, false],
    [1, 2.5, false],
  ]) {
    const warnings = []
    const query = mountSettings(settings(), message => warnings.push(message))
    try {
      await query.state.loadSettings()
      assert.equal(query.state.form.requestTuning.maxWaitingPerKey, 0)
      assert.equal(query.state.form.requestTuning.keyQueueTimeoutSeconds, 30)
      Object.assign(query.state.form.requestTuning, { maxWaitingPerKey, keyQueueTimeoutSeconds })
      await query.state.saveSettings()
      assert.equal(query.requests.length, valid ? 1 : 0)
      assert.equal(warnings.length, valid ? 0 : 1)
      if (valid) {
        await query.state.loadSettings()
        assert.equal(query.state.form.requestTuning.maxWaitingPerKey, maxWaitingPerKey)
        assert.equal(query.state.form.requestTuning.keyQueueTimeoutSeconds, keyQueueTimeoutSeconds)
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
      requestTuningDefaults: { websocketMaxAgeMs: 1 },
      openaiWebsocketPool: { ...settings().openaiWebsocketPool, maxAgeMs: 60000 },
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
      assert.equal(Object.hasOwn(query.state.form.requestTuning, 'websocketHttpFallbackEnabled'), false)
      assert.equal(query.state.form.openaiWebsocketPool.maxAgeMs, 60_000)
      assert.equal(Object.hasOwn(query.state.form.requestTuning, 'websocketMaxAgeMs'), false)
      assert.equal(query.state.form.requestTuning.websocketMaxRetries, 5)
      assert.equal(Object.hasOwn(query.state.form.requestTuning, 'websocketMaxConnecting'), false)

      query.state.form.requestTuning.websocketMaxRetries = 2
      await query.state.saveSettings()
      assert.equal(query.requests.length, 1)
      assert.equal(Object.hasOwn(query.requests[0].requestTuning, 'websocketMaxConnecting'), false)
      assert.equal(Object.hasOwn(query.requests[0].requestTuning, 'websocketHttpFallbackEnabled'), false)
      assert.equal(query.requests[0].requestTuning.websocketMaxRetries, 2)
      assert.equal(query.requests[0].maxConcurrentPerAccount, 5)
      assert.deepEqual(query.requests[0].modelMappings, initial.modelMappings)

      await query.state.loadSettings()
      await query.state.saveSettings()
      assert.equal(query.requests.length, 2)
      assert.deepEqual(query.requests[1], { ...query.requests[0], configRevision: 8 })
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
      accountErrorProtection: { enabled: false, rules: [], threshold: 12, windowSeconds: 600, durationSeconds: 7200, probeEnabled: true, probeModel: null },
      smartScheduling: { loadWeight: 1, quotaWeight: 0.8, healthWeight: 1, latencyWeight: 0.5, resetWeight: 0, queueWeight: 0, preferHigherWeight: false, resetWindowWeights: { fiveHour: 0, weekly: 0, monthly: 0 } },
      maxAccountSwitches: 31,
      maxRequestAttempts: 32,
      websocketMaxRetries: 5,
      streamPrefetchBytes: 128 * 1024,
      websocketFailureThreshold: 3,
      websocketFailureWindowMs: 30_000,
      websocketFailureOpenDurationMs: 30_000,
      rateLimitCooldownSeconds: 60,
      maxRetryAfterSeconds: 240,
      openaiLocationOverrideEnabled: false,
      openaiRequestLocation: { country: 'US', region: 'Ohio', city: 'Piketon', timezone: 'America/New_York' },
      maxWaitingPerKey: 0,
      keyQueueTimeoutSeconds: 30,
      switchAccountOnQueueFailure: false,
      accountBusyWaitEnabled: false,
      maxWaitingPerAccount: 10,
      accountQueueTimeoutSeconds: 30,
    })
  }
  finally {
    query.stop()
  }
})

const busyWaitDefaults = {
  accountBusyWaitEnabled: false,
  maxWaitingPerAccount: 10,
  accountQueueTimeoutSeconds: 30,
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
    maxWaitingPerAccount: 7,
    accountQueueTimeoutSeconds: 90,
  }
  const editedWait = {
    accountBusyWaitEnabled: true,
    maxWaitingPerAccount: 9,
    accountQueueTimeoutSeconds: 120,
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
    assert.deepEqual(query.requests[1], { ...query.requests[0], configRevision: 8 })
  }
  finally {
    query.stop()
  }
})

test('account busy wait defaults inherit per field and preserve an explicit false', async () => {
  const serverDefaults = {
    accountBusyWaitEnabled: true,
    maxWaitingPerAccount: 5,
    accountQueueTimeoutSeconds: 75,
  }
  for (const { defaults, tuning, expected } of [
    { defaults: undefined, tuning: undefined, expected: busyWaitDefaults },
    { defaults: serverDefaults, tuning: undefined, expected: serverDefaults },
    {
      defaults: serverDefaults,
      tuning: { accountBusyWaitEnabled: false, maxWaitingPerAccount: 8 },
      expected: { ...serverDefaults, accountBusyWaitEnabled: false, maxWaitingPerAccount: 8 },
    },
    {
      defaults: { accountQueueTimeoutSeconds: 75 },
      tuning: { accountBusyWaitEnabled: true, maxWaitingPerAccount: 250 },
      expected: {
        ...busyWaitDefaults,
        accountBusyWaitEnabled: true,
        accountQueueTimeoutSeconds: 75,
        maxWaitingPerAccount: 250,
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
    maxWaitingPerAccount: 11,
    accountQueueTimeoutSeconds: 105,
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
    for (const [maxWaiting, timeoutSeconds] of [[1, 1], [1000, 120]]) {
      const expected = {
        accountBusyWaitEnabled: enabled,
        maxWaitingPerAccount: maxWaiting,
        accountQueueTimeoutSeconds: timeoutSeconds,
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
  ['maxWaitingPerAccount', 1000],
  ['accountQueueTimeoutSeconds', 120],
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
        assert.deepEqual(warnings, invalidValues.map(() => '账号忙时等待人数须为 1–1000 的整数，等待秒数须为 1–120 的整数'))

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
  assert.doesNotMatch(component, /websocketHttpFallbackEnabled/)
  assert.doesNotMatch(api, /websocketHttpFallbackEnabled/)
})

test('error protection is opt-in, validates rules, and saves independently from scheduling', async () => {
  const warnings = []
  const query = mountSettings(settings(), message => warnings.push(message))
  try {
    await query.state.loadSettings()
    const original = JSON.stringify(query.state.form.requestTuning)
    assert.equal(query.state.form.accountErrorProtection.enabled, false)
    query.state.form.accountErrorProtection.enabled = true
    await query.state.saveSettings()
    assert.equal(query.requests.length, 0)
    assert.equal(warnings.length, 1)
    query.state.form.accountErrorProtection.rules = [{
      id: 'test',
      name: 'Synthetic failures',
      conditions: [{ field: 'status', operator: 'equals', values: ['503'], ignoreCase: false }],
    }]
    query.state.form.accountErrorProtection.threshold = 3
    await query.state.saveSettings()
    assert.equal(query.requests.length, 1)
    const policy = query.requests[0].requestTuning.accountErrorProtection
    assert.equal(policy.enabled, true)
    assert.equal(policy.threshold, 3)
    assert.equal(policy.probeModel, null)
    assert.equal(policy.rules[0].conditions[0].values[0], '503')
    assert.equal(JSON.stringify(query.state.form.requestTuning), original)
  }
  finally { query.stop() }
})

for (const status of [400, 403, 409, 422]) {
  test(`structured settings rejection ${status} preserves the draft without reloading`, async () => {
    let reads = 0
    const query = mountSettings(settings(), assert.fail, {
      getSettings: async () => {
        reads += 1
        return settings()
      },
      updateSettings: async () => { throw new apiErrors.ApiError('Rejected', status, status * 100) },
    })
    try {
      await query.state.loadSettings()
      query.state.form.requestIntervalMs = 123
      await query.state.saveSettings()
      await vue.nextTick()
      assert.equal(reads, 1)
      assert.equal(query.state.form.requestIntervalMs, 123)
      assert.equal(query.state.hasChanges.value, true)
    }
    finally { query.stop() }
  })
}

test('quota reset weights persist independently across strategies and validate complete numeric drafts', async () => {
  const warnings = []
  const query = mountSettings(settings(), message => warnings.push(message))
  try {
    await query.state.loadSettings()
    const smart = JSON.stringify(query.state.form.requestTuning.smartScheduling)
    const weights = { fiveHour: 0.1, weekly: 10, monthly: 2.5 }
    Object.assign(query.state.form.quotaResetWindowWeights, weights)
    for (const strategy of ['quota_reset_priority', 'smart', 'sticky', 'quota_reset_priority']) {
      query.state.form.rotationStrategy = strategy
      await query.state.saveSettings()
      await query.state.loadSettings()
      assert.deepEqual(query.requests.at(-1).quotaResetWindowWeights, weights)
      assert.equal(Object.hasOwn(query.requests.at(-1), 'quotaResetWindowWeightsDefaults'), false)
      assert.equal(JSON.stringify(query.state.form.requestTuning.smartScheduling), smart)
    }
    const saved = query.requests.length
    for (const field of ['fiveHour', 'weekly', 'monthly']) {
      for (const invalid of [-0.1, 10.1, 0.01, Number.NaN, Infinity, '', null, undefined]) {
        query.state.form.quotaResetWindowWeights[field] = invalid
        await query.state.saveSettings()
        assert.equal(query.requests.length, saved)
      }
      query.state.form.quotaResetWindowWeights[field] = weights[field]
    }
    assert.equal(warnings.length, 24)
    await query.state.saveSettings()
    assert.equal(query.requests.length, saved + 1)
  }
  finally { query.stop() }
})

test('OpenAI platform settings roundtrip cleared headers and reject invalid connection budgets', async () => {
  const warnings = []
  const query = mountSettings(settings(), value => warnings.push(value))
  try {
    await query.state.loadSettings()
    query.state.form.openaiRequestSettings.residency = null
    query.state.form.openaiRequestSettings.accountRoutingOverride = 'us_cr'
    query.state.form.openaiRequestSettings.oauthGuardianReviewModel = false
    query.state.form.openaiWebsocketPool.enabled = false
    query.state.form.openaiWebsocketPool.fastPathBudgetMs = 1234
    await query.state.saveSettings()
    assert.equal(query.requests.at(-1).openaiRequestSettings.residency, null)
    assert.equal(query.requests.at(-1).openaiRequestSettings.accountRoutingOverride, 'us_cr')
    assert.equal(query.requests.at(-1).openaiWebsocketPool.enabled, false)
    await query.state.loadSettings()
    assert.equal(query.state.form.openaiWebsocketPool.fastPathBudgetMs, 1234)
    for (const [key, value] of [['fastPathBudgetMs', 0], ['fastPathBudgetMs', 15001], ['maxAgeMs', 0], ['idleTimeoutMs', -1], ['streamIdleTimeoutMs', 0.5], ['maxAgeMs', Number.NaN]]) {
      const previous = query.state.form.openaiWebsocketPool[key]
      query.state.form.openaiWebsocketPool[key] = value
      await query.state.saveSettings()
      assert.equal(query.requests.length, 1)
      query.state.form.openaiWebsocketPool[key] = previous
    }
    assert.equal(warnings.length, 6)
    query.state.form.openaiWebsocketPool.streamIdleTimeoutMs = 0
    await query.state.saveSettings()
    assert.equal(query.requests.at(-1).openaiWebsocketPool.streamIdleTimeoutMs, 0)
  }
  finally { query.stop() }
})

test('retired Excel settings are discarded while native retry and scheduling controls survive', async () => {
  const initial = settings()
  initial.excelDefaultModels = ['legacy-model']
  initial.requestTuning = { ...initial.requestTuning, maxRequestAttempts: 7, websocketMaxRetries: 3, excelImageTransport: { mode: 'relay', publicUrl: 'https://images.example.com' }, excelImageMaxCount: 20 }
  const query = mountSettings(initial)
  try {
    await query.state.loadSettings()
    await query.state.saveSettings()
    const saved = query.requests[0]
    assert.equal('excelDefaultModels' in saved, false)
    assert.equal(Object.keys(saved.requestTuning).some(key => key.startsWith('excel')), false)
    assert.equal(saved.requestTuning.maxRequestAttempts, 7)
    assert.equal(saved.requestTuning.websocketMaxRetries, 3)
    assert.equal(saved.maxConcurrentPerAccount, initial.maxConcurrentPerAccount)
    assert.equal(saved.rotationStrategy, initial.rotationStrategy)
  }
  finally { query.stop() }
})
