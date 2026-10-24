/* eslint-disable test/no-import-node-test -- Node's built-in regression runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

const filename = new URL('../src/views/accounts/components/AccountQuotaOverdraft/presenter.ts', import.meta.url)
const { outputText } = ts.transpileModule(readFileSync(filename, 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
})
const presenter = {}
runInNewContext(outputText, {
  exports: presenter,
  require: name => name === '@/utils/date'
    ? { formatDateTime: value => `DATE:${value}` }
    : {},
}, { filename: String(filename) })

function state(status, overrides = {}) {
  return {
    status,
    quotaWindow: 'multiple',
    probeModel: 'gpt-6-astra',
    attempts: 2,
    attemptLimit: 5,
    reasonCode: null,
    testedAt: '2026-10-24T00:00:00Z',
    retryAt: null,
    recoverAt: null,
    windows: [],
    ...overrides,
  }
}

test('overdraft status presenter keeps the account list compact', () => {
  assert.deepEqual(
    JSON.parse(JSON.stringify(presenter.codexQuotaOverdraftStatusView(state('pending'), Date.parse('2026-10-24T00:00:00Z')))),
    {
      label: '额度探测中',
      detail: '2/5 · gpt-6-astra',
      title: '5h / 7d · gpt-6-astra · 最近探测：DATE:2026-10-24T00:00:00Z',
      tone: 'warning',
    },
  )
  assert.equal(presenter.codexQuotaOverdraftStatusView(state('passed'), 0).label, '透支调度中')
  assert.equal(presenter.codexQuotaOverdraftStatusView(state('recovered'), 0).label, '额度已恢复')
  assert.equal(presenter.codexQuotaOverdraftStatusView(state('pending', { attempts: 0 }), 0).label, '等待探测')
})

test('inconclusive probe shows retry countdown and model-unavailable remains in tooltip', () => {
  const view = presenter.codexQuotaOverdraftStatusView(state('inconclusive', {
    attempts: 3,
    reasonCode: 'model_unavailable',
    retryAt: '2026-10-24T00:00:42Z',
  }), Date.parse('2026-10-24T00:00:00Z'))
  assert.equal(view.label, '探测无结论')
  assert.equal(view.detail, '3/5 · 42 秒后重试')
  assert.ok(view.title.includes('探测模型不可用'))
})

test('window stats use server-provided displays and exact window keys', () => {
  const window = {
    windowKey: 'codex-5h',
    active: true,
    startedAt: null,
    recoverAt: null,
    stats: {
      requests: 28,
      requestsDisplay: '28 次',
      tokens: 321000,
      tokensDisplay: '321K',
      costUsd: '1.46',
      costUsdDisplay: '$1.46',
    },
  }
  const probe = state('passed', { windows: [window] })
  assert.equal(presenter.codexQuotaOverdraftWindow(probe, 'codex-5h').active, true)
  assert.equal(presenter.codexQuotaOverdraftWindow(probe, 'codex-7d'), null)
  assert.equal(presenter.codexQuotaOverdraftStatsText(window), '28 次 · 321K · $1.46')
})
