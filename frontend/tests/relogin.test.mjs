/* eslint-disable test/no-import-node-test -- this suite uses Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

function load(path, dependencies = {}) {
  const exports = {}
  const { outputText } = ts.transpileModule(readFileSync(new URL(path, import.meta.url), 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, { exports, TextEncoder, require: name => dependencies[name] })
  return exports
}
const { importPreview } = load('../src/views/relogin/import-preview.ts')
const { processingStatus, recoveryCountdown, retryProgress, credentialLabel, poolPresentation, matchesPool, workspaceChoices, shortWorkspace } = load('../src/views/relogin/presentation.ts')

test('terminal relogin failures show manual handling and retries never hide uncertain pushes', () => {
  const row = {
    status: 'failed',
    message: '原工作区不可访问',
    recovery: { state: 'manual_required', message: '自动重登已停止', retryAt: null, retriesUsed: 0, maxRetries: 2 },
  }
  assert.equal(processingStatus(row).label, '需人工处理')
  assert.match(processingStatus(row).detail, /原工作区不可访问/)
  assert.equal(retryProgress(row), '已重试 0/2')
  row.recovery.state = 'retry_limit'
  row.recovery.maxRetries = 0
  assert.equal(retryProgress(row), '已重试 0/0')
  row.status = 'uncertain'
  assert.equal(processingStatus(row).label, '推送待核实')
  assert.equal(retryProgress(row), '')
  row.status = 'pushing'
  assert.equal(retryProgress(row), '')
  row.status = 'failed'
  delete row.recovery.maxRetries
  assert.equal(retryProgress(row), '')
})

test('relogin settings preserve explicit zero and omit retry fields on legacy concurrency updates', async () => {
  const calls = []
  const { configureRelogin } = load('../src/api/modules/relogin.ts', {
    '../request': async config => calls.push(config),
  })
  await configureRelogin({ concurrency: 1, paused: false, maxRetries: 0, retryIntervalMinutes: 17 })
  assert.deepEqual(JSON.parse(JSON.stringify(calls[0].data)), {
    concurrency: 1,
    paused: false,
    maxRetries: 0,
    retryIntervalMinutes: 17,
  })
  await configureRelogin({ concurrency: 2, paused: true })
  assert.equal(Object.hasOwn(calls[1].data, 'maxRetries'), false)
  assert.equal(Object.hasOwn(calls[1].data, 'retryIntervalMinutes'), false)
})

test('recovery diagnostics supersede old sync messages but never an active or uncertain push', () => {
  const row = {
    status: 'ready',
    poolStatus: 'synced',
    message: '凭据已同步到号池',
    recovery: { state: 'cooldown', message: '等待重试', retryAt: '2026-01-01T00:05:00Z' },
  }
  assert.equal(processingStatus(row).key, 'cooldown')
  assert.equal(recoveryCountdown(row, Date.parse('2026-01-01T00:03:59Z')), '剩余 1分01秒')
  assert.equal(recoveryCountdown(row, Date.parse('2026-01-01T00:05:00Z')), '等待下一轮检查')
  row.recovery.retryAt = 'invalid'
  assert.equal(recoveryCountdown(row, 0), '')
  for (const status of ['running', 'pushing', 'uncertain', 'queued']) {
    row.status = status
    assert.equal(processingStatus(row).key, status)
  }
  row.status = 'ready'
  for (const state of ['waiting', 'loop_guard', 'retry_limit', 'workspace_required', 'disabled', 'paused', 'account_disabled']) {
    row.recovery.state = state
    assert.equal(processingStatus(row).key, state)
  }
  row.recovery.state = 'idle'
  assert.equal(processingStatus(row).key, 'synced')
  delete row.recovery
  assert.equal(processingStatus(row).key, 'synced')
})

test('relogin view never treats cached verification or previous push as current pool health', () => {
  const row = {
    status: 'uncertain',
    credentialStatus: 'verified',
    poolStatus: 'pending_push',
    poolAccountIds: ['account'],
    reloginAccountId: 'account',
    poolAccounts: [{ id: 'account', workspaceId: 'workspace', enabled: false, status: 'error', errorReason: 'credential_expired' }],
  }
  assert.equal(processingStatus(row).label, '推送待核实')
  assert.equal(credentialLabel(row), '新凭据已验证')
  assert.equal(poolPresentation(row).label, '凭据失效')
  assert.equal(poolPresentation(row).caption, '暂停调度')
  assert.equal(matchesPool(row, 'error'), true)
  assert.equal(matchesPool(row, 'disabled'), true)
  row.status = 'ready'
  row.poolStatus = 'synced'
  assert.equal(processingStatus(row).label, '已同步到号池')
  assert.match(poolPresentation(row).label, /凭据失效/)
  row.poolAccounts[0].enabled = true
  row.poolAccounts[0].status = 'normal'
  assert.equal(poolPresentation(row).label, '正常')
  delete row.poolAccounts
  assert.equal(poolPresentation(row).label, '状态未知')
  assert.equal(credentialLabel({ credentialStatus: 'none' }), '尚未获取')
})

test('unresolved workspaces expose every matching pool status without claiming one is normal', () => {
  const row = {
    poolAccountIds: ['a', 'b'],
    poolAccounts: [
      { id: 'a', enabled: true, status: 'normal' },
      { id: 'b', enabled: false, status: 'error', errorReason: 'credential_expired' },
    ],
    reloginAccountId: null,
  }
  assert.equal(poolPresentation(row).label, '2 个池中账号')
  assert.equal(matchesPool(row, 'error'), true)
  assert.equal(matchesPool(row, 'disabled'), true)
  row.reloginAccountId = 'a'
  assert.equal(matchesPool(row, 'error'), false)
  assert.equal(poolPresentation(row).label, '正常')
})

test('relogin workspace choices are deduplicated known IDs, not arbitrary preferred text', () => {
  const row = {
    poolAccounts: [{ workspaceId: 'known', planType: 'team' }, { workspaceId: 'known', planType: 'team' }],
    workspaceId: 'cached',
    planType: 'free',
    preferredWorkspaceId: 'unverified',
  }
  assert.deepEqual(Array.from(workspaceChoices(row), item => item.value), ['known', 'cached'])
  assert.equal(shortWorkspace('12345678-1234-5678-9012-123456789012'), '12345678…789012')
})

test('relogin preview retains source line numbers, normalizes email and never returns secrets', () => {
  const rows = importPreview('\uFEFFTest@Example.invalid----p%----ss----jbsw y3dp-ehpk3pxp\n\nsecond@example.invalid----p----JBSWY3DPEHPK3PXP')
  assert.equal(rows.length, 2)
  assert.equal(rows[0].email, 'test@example.invalid')
  assert.equal(rows[0].valid, true)
  assert.equal(rows[1].line, 3)
  assert.equal(JSON.stringify(rows).includes('JBSWY'), false)
  assert.equal(JSON.stringify(rows).includes('p%'), false)
})

test('relogin preview rejects malformed secrets, empty passwords and case-folded duplicates', () => {
  const invalid = [
    'bad----p----JBSWY3DPEHPK3PXP',
    'test@example.invalid--------JBSWY3DPEHPK3PXP',
    'test@example.invalid----p----JBSWY3DPEHPK3PXP0',
    'test@example.invalid----p----JBSWY3DPEHPK3P',
  ]
  for (const text of invalid)
    assert.equal(importPreview(text)[0].valid, false)
  const repeated = importPreview('test@example.invalid----p----JBSWY3DPEHPK3PXP\nTEST@example.invalid----p----JBSWY3DPEHPK3PXP')
  assert.equal(repeated[1].valid, false)
})

test('relogin preview rejects mailbox formats but accepts Outlook accounts with TOTP', () => {
  for (const text of [
    'person@outlook.com----test-only-password----123e4567-e89b-12d3-a456-426614174000',
    'person@outlook.com----test-only-password----123e4567-e89b-12d3-a456-426614174000----!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!',
    'person@outlook.com----test-only-password----123e4567-e89b-12d3-a456-426614174000----JBSWY3DPEHPK3PXP',
    'person@outlook.com|synthetic-mailbox-token|123e4567-e89b-12d3-a456-426614174000',
  ]) {
    const rows = importPreview(text)
    assert.equal(rows[0].valid, false)
    assert.equal(JSON.stringify(rows).includes('test-only-password'), false)
    assert.equal(JSON.stringify(rows).includes('synthetic-mailbox-token'), false)
  }
  const rows = importPreview('person@outlook.com----p----JBSWY3DPEHPK3PXP')
  assert.equal(rows[0].valid, true)
  assert.equal(rows[0].email, 'person@outlook.com')
})

test('relogin push sends exactly the confirmed row versions and does not replay', async () => {
  const calls = []
  const { pushRelogin } = load('../src/api/modules/relogin.ts', {
    '../request': async (config) => {
      calls.push(config)
      throw new Error('version changed')
    },
  })
  await assert.rejects(pushRelogin([{ id: 'a', revision: 7 }, { id: 'b', revision: 12 }]), /version changed/)
  assert.equal(calls.length, 1)
  assert.deepEqual(JSON.parse(JSON.stringify(calls[0].data)), {
    ids: ['a', 'b'],
    revisions: { a: 7, b: 12 },
  })
})
