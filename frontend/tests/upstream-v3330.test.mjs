/* eslint-disable test/no-import-node-test -- uses the established Node regression runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

function load(path, dependencies = {}) {
  const exports = {}
  const { outputText } = ts.transpileModule(readFileSync(new URL(path, import.meta.url), 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, { exports, require(name) {
    assert.ok(name in dependencies, `unexpected dependency: ${name}`)
    return dependencies[name]
  } })
  return exports
}

test('saving one backup form preserves unsaved values in the other form', async () => {
  const server = {
    storageRevision: 1,
    verified: true,
    endpoint: 'https://backup.example.invalid',
    region: 'auto',
    bucket: 'fixture',
    accessKeyId: 'synthetic',
    secretAccessKey: 'synthetic',
    prefix: 'backups/',
    forcePathStyle: false,
    scheduleEnabled: false,
    cronExpression: '0 2 * * *',
    scheduleTimezone: 'UTC',
    retentionDays: 7,
    retentionCount: 7,
  }
  const { useBackupSettings } = load('../src/views/settings/composables/useBackupSettings.ts', {
    vue,
    '@/api': {
      getBackupSettings: async () => ({ ...server }),
      updateBackupStorage: async (data) => {
        Object.assign(server, data, { storageRevision: server.storageRevision + 1 })
        return { ...server }
      },
      updateBackupSchedule: async (data) => {
        Object.assign(server, data)
        return { ...server }
      },
    },
    '@/components/base/BaseToast': { toast: { success() {}, error() {} } },
    '@/utils/async': { errorMessage: String },
  })
  const state = useBackupSettings()
  await state.load()
  assert.equal(state.hasChanges.value, false)
  state.schedule.cronExpression = '0 3 * * *'
  state.storage.prefix = 'new-prefix/'
  assert.equal(state.hasChanges.value, true)
  assert.equal(await state.saveStorage(), true)
  assert.equal(state.schedule.cronExpression, '0 3 * * *')
  assert.equal(state.hasChanges.value, true)
  state.storage.bucket = 'unsaved-bucket'
  assert.equal(await state.saveSchedule(), true)
  assert.equal(state.storage.bucket, 'unsaved-bucket')
  assert.equal(state.hasChanges.value, true)
  state.storage.bucket = 'fixture'
  assert.equal(state.hasChanges.value, false)
})

test('inline API key validation keeps existing optional and numeric limits', () => {
  const { apiKeyFormErrors } = load('../src/views/api-keys/utils/formValidation.ts')
  const form = { name: 'synthetic', dailyLimitUsd: '', weeklyLimitUsd: '', maxConcurrency: '', requestsPerMinute: '' }
  assert.deepEqual(Object.keys(apiKeyFormErrors(form)), [])
  assert.ok(apiKeyFormErrors({ ...form, dailyLimitUsd: '-1' }).dailyLimitUsd)
  assert.ok(apiKeyFormErrors({ ...form, weeklyLimitUsd: '1.12345678901' }).weeklyLimitUsd)
  assert.ok(apiKeyFormErrors({ ...form, maxConcurrency: '1.5' }).maxConcurrency)
  assert.ok(apiKeyFormErrors({ ...form, requestsPerMinute: '9007199254740992' }).requestsPerMinute)
  assert.ok(apiKeyFormErrors({ ...form, name: ' ' }).name)
  assert.deepEqual(Object.keys(apiKeyFormErrors({ ...form, weeklyLimitUsd: '0.1234567890', maxConcurrency: '0' })), [])
})
