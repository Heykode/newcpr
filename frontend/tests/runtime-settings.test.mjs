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
    exports,
    require: name => dependencies[name] ?? (name === '@/utils/excel-defaults' ? loadModule(new URL('../src/utils/excel-defaults.ts', import.meta.url)) : require(name)),
  }, { filename: String(filename) })
  return exports
}

const apiErrors = loadModule(new URL('../src/api/error.ts', import.meta.url))
const asyncUtils = loadModule(new URL('../src/utils/async.ts', import.meta.url))

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
      maxAccountSwitches: 31,
      maxRequestAttempts: 32,
      websocketMaxRetries: 5,
      websocketHttpFallbackEnabled: true,
      websocketLargeRequestThresholdBytes: 15 * 1024 * 1024,
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
      excelImageMaxBytes: 4 * 1024 * 1024,
      excelImageTotalBytes: 6 * 1024 * 1024,
      excelImageMaxCount: 16,
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
      ['excelImageRelayBytes', 2 * 1024 * 1024],
      ['excelImageRelayRequests', 8],
      ['excelImageRelayDownloads', 4],
      ['excelImageRelayEntries', 16],
      ['excelImageMaxBytes', 8 * 1024 * 1024],
      ['excelImageTotalBytes', 16 * 1024 * 1024],
      ['excelImageMaxCount', 32],
    ]) {
      query.state.form.requestTuning[field] = value
    }
    await query.state.saveSettings()
    await query.state.loadSettings()
    assert.equal(query.state.form.requestTuning.excelImageRelayBytes, 2 * 1024 * 1024)
    assert.equal(query.state.form.requestTuning.excelImageRelayDownloads, 4)
    assert.equal(query.state.form.requestTuning.excelImageRelayRequests, 8)
    assert.equal(query.state.form.requestTuning.excelImageRelayEntries, 16)
    assert.equal(query.state.form.requestTuning.excelImageMaxBytes, 8 * 1024 * 1024)
    assert.equal(query.state.form.requestTuning.excelImageTotalBytes, 16 * 1024 * 1024)
    assert.equal(query.state.form.requestTuning.excelImageMaxCount, 32)
    assert.equal(query.requests[0].requestTuning.rateLimitCooldownSeconds, 60)
    assert.equal(query.requests[0].maxConcurrentPerAccount, 5)
    for (const [field, invalid] of [
      ['excelImageRelayBytes', 1024],
      ['excelImageRelayBytes', 2048 * 1024 * 1024 + 1],
      ['excelImageRelayDownloads', 0],
      ['excelImageRelayDownloads', 129],
      ['excelImageRelayRequests', 0],
      ['excelImageRelayRequests', 513],
      ['excelImageRelayEntries', 0],
      ['excelImageRelayEntries', 4097],
      ['excelImageMaxBytes', 0],
      ['excelImageMaxBytes', 20 * 1024 * 1024 + 1],
      ['excelImageTotalBytes', 0],
      ['excelImageTotalBytes', 32 * 1024 * 1024 + 1],
      ['excelImageMaxCount', 0],
      ['excelImageMaxCount', 4097],
    ]) {
      const before = query.state.form.requestTuning[field]
      query.state.form.requestTuning[field] = invalid
      await query.state.saveSettings()
      assert.equal(query.requests.length, 1)
      query.state.form.requestTuning[field] = before
    }
    assert.equal(warnings.length, 14)
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
