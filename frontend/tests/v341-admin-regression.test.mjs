/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

const require = createRequire(import.meta.url)

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

test('quota reset uses UUIDv4 without randomUUID and reuses it after an uncertain retry', async () => {
  const resetModulePath = new URL('../src/views/accounts/composables/useAccountResetCredits.ts', import.meta.url)
  const consumedRequests = []
  const credit = {
    id: 'credit_test',
    title: '测试重置',
    status: 'available',
    expiresAt: null,
  }
  class MockApiError extends Error {
    constructor(message, fields) {
      super(message)
      Object.assign(this, fields)
    }
  }
  let consumeAttempt = 0
  const api = {
    getAccountResetCredits: async () => ({ credits: [credit], availableCount: 1 }),
    consumeAccountResetCredit: async (request) => {
      consumedRequests.push(request)
      consumeAttempt += 1
      if (consumeAttempt === 1)
        throw new MockApiError('connection lost', { status: 0, kind: 'network', code: 50202 })
      return { code: 'reset' }
    },
    refreshAccountQuota: async () => ({ account: {} }),
  }
  const module = loadModule(resetModulePath, {
    vue,
    '@/api': api,
    '@/api/request': { ApiError: MockApiError },
    '@/components/base/BaseToast': {
      toast: {
        error: assert.fail,
        success: () => {},
        warning: () => {},
      },
    },
    '@/utils/async': { errorMessage: error => error.message },
  }, {
    AbortController,
    crypto: {
      getRandomValues(bytes) {
        for (let index = 0; index < bytes.length; index += 1)
          bytes[index] = index
        return bytes
      },
    },
  })
  const scope = vue.effectScope()
  try {
    const query = scope.run(() => module.useAccountResetCredits({
      accountId: () => 'account_test',
      onAccountUpdated: () => {},
    }))
    await query.loadCredits()
    query.selectCredit('credit_test')
    query.requestConsume()
    assert.equal(query.showConfirm.value, true)

    await query.confirmConsume()
    assert.equal(consumedRequests.length, 1)
    assert.match(
      consumedRequests[0].redeemRequestId,
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    )
    assert.equal(query.ambiguous.value, true)

    query.requestConsume()
    await query.confirmConsume()
    assert.equal(consumedRequests.length, 2)
    assert.equal(
      consumedRequests[1].redeemRequestId,
      consumedRequests[0].redeemRequestId,
      'an uncertain retry must preserve the idempotency key',
    )
  }
  finally {
    scope.stop()
  }
})

test('budget and account usage regressions preserve exact details and compatibility fields', async () => {
  const budgetFormat = loadModule(new URL('../src/views/api-keys/utils/format.ts', import.meta.url), {})
  assert.equal(budgetFormat.formatBudgetAmount('1234.5000'), '1,234.5')
  assert.equal(budgetFormat.formatBudgetAmount('0.009'), '0.01')
  assert.equal(budgetFormat.formatBudgetAmount('999999999999999999.999'), '1,000,000,000,000,000,000')
  assert.equal(budgetFormat.formatBudgetAmount('not-a-number'), 'not-a-number')

  const date = loadModule(new URL('../src/utils/date.ts', import.meta.url), {})
  assert.equal(
    date.formatDateTime('2026-09-10T16:30:45.000Z', '—', 'Asia/Shanghai'),
    '2026-09-11 00:30:45',
  )

  const budgetCell = readFileSync(new URL('../src/views/api-keys/components/ApiKeyBudgetCell.vue', import.meta.url), 'utf8')
  const apiKeys = readFileSync(new URL('../src/api/modules/api-keys.ts', import.meta.url), 'utf8')
  const usagePanel = readFileSync(new URL('../src/views/accounts/components/AccountUsagePanel.vue', import.meta.url), 'utf8')

  assert.match(budgetCell, /formatBudgetAmount\(window\.used\)/)
  assert.match(budgetCell, /\$\{\{ window\.used \}\}/)
  assert.match(budgetCell, /formatDateTime\(window\.reset, '—', 'Asia\/Shanghai'\)/)
  assert.doesNotMatch(budgetCell, /费用核账|待核账|unresolvedRequests|reconcile/)
  assert.doesNotMatch(apiKeys, /UnresolvedClientCharge|getUnresolvedClientCharges|reconcileClientCharge|unresolvedRequests/)
  assert.match(usagePanel, /account\.cumulativeCosts \?\? \[\]/)
  assert.match(usagePanel, /account\.usage\.windowLabelDisplay/)
})
