/* eslint-disable test/no-import-node-test -- uses the project's Node test runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

const require = createRequire(import.meta.url)
function load(path, dependencies = {}) {
  const exports = {}
  const { outputText } = ts.transpileModule(readFileSync(new URL(path, import.meta.url), 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, {
    exports,
    TextEncoder,
    AbortController,
    require: name => dependencies[name] ?? (
      name.endsWith('/utils/purchaseCost')
        ? load('../src/views/accounts/utils/purchaseCost.ts')
        : name.startsWith('@/utils/')
          ? load(`../src/utils/${name.slice('@/utils/'.length)}.ts`, dependencies)
          : require(name)),
  })
  return exports
}
const cost = load('../src/views/accounts/utils/purchaseCost.ts')
const plain = value => JSON.parse(JSON.stringify(value))
const saved = { amountCny: '50.0000000000', cycleAnchor: '2026-01-31', periodStart: '2026-09-30', periodEnd: '2026-10-31', usageUsd: '200.0000000000', breakevenCnyPerUsd: '0.25', historyComplete: true }

test('purchase inputs preserve decimals, support explicit clearing and reject invalid dates', () => {
  assert.deepEqual(plain(cost.purchaseCostPatch(' 50.0000000001 ', '2026-01-31')), { amountCny: '50.0000000001', cycleStart: '2026-01-31' })
  assert.deepEqual(plain(cost.purchaseCostPatch('', '')), { amountCny: null })
  assert.deepEqual(plain(cost.purchaseCostPatch('0', '')), { amountCny: '0' })
  for (const invalid of ['-1', '1e3', 'NaN', 'Infinity', '0.00000000001', '10000000000'])
    assert.throws(() => cost.purchaseCostPatch(invalid, ''))
  for (const date of ['2026-02-30', '2100-01-01', '1999-01-01', '2026-2-1'])
    assert.throws(() => cost.purchaseCostPatch('50', date))
  assert.equal(cost.chinaToday(new Date('2026-09-30T16:00:00Z')), '2026-10-01')
})

test('breakeven presentation distinguishes unset, zero use, free and incomplete history', () => {
  for (const value of [undefined, null, { ...saved, amountCny: null }, { ...saved, breakevenCnyPerUsd: null }])
    assert.equal(cost.purchaseCostLabel(value), '—')
  assert.equal(cost.purchaseCostLabel(saved), '0.25')
  assert.equal(cost.purchaseCostLabel({ ...saved, amountCny: '0', breakevenCnyPerUsd: '0' }), '0')
  assert.equal(cost.purchaseCostLabel({ ...saved, breakevenCnyPerUsd: '0.000000001' }), '<0.000001')
  assert.match(cost.purchaseCostTitle(saved), /每账号月成本：50 元/)
  assert.match(cost.purchaseCostTitle(saved), /200 美元/)
  assert.match(cost.purchaseCostTitle(saved), /2026-09-30 至 2026-10-31（不含结束日，上海时间）/)
  assert.match(cost.purchaseCostTitle(saved), /不换汇/)
  assert.match(cost.purchaseCostTitle({ ...saved, historyComplete: false }), /历史记录不完整/)
})

test('import omits unspecified costs and gives each account the exact entered amount', () => {
  const scheduling = load('../src/views/accounts/utils/schedulingForm.ts')
  const creation = load('../src/views/accounts/components/AccountCreateModal/model.ts', {
    '../../utils/schedulingForm': scheduling,
    '@/views/accounts/utils/schedulingForm': scheduling,
    '../../utils/modelAccess': load('../src/views/accounts/utils/modelAccess.ts'),
  })
  const form = creation.emptyAccountCreateForm()
  form.provider = 'openai'
  assert.equal('purchaseCost' in creation.accountImportSettings(form), false)
  form.purchaseAmount = '50'
  form.purchaseCycleStart = '2026-01-31'
  assert.deepEqual(plain(creation.accountImportSettings(form).purchaseCost), { amountCny: '50', cycleStart: '2026-01-31' })
  form.purchaseAmount = ''
  assert.equal('purchaseCost' in creation.accountImportSettings(form), false)
})

test('single account editor preserves omitted costs, sends changes and clears without resetting the date', async (t) => {
  const updates = []
  const errors = []
  const toast = { warning() {}, success() {}, error: value => errors.push(value) }
  const scheduling = load('../src/views/accounts/utils/schedulingForm.ts')
  const asyncAction = load('../src/composables/useAsyncAction.ts', {
    vue,
    '@/api/request': load('../src/api/error.ts'),
    '@/components/base/BaseToast': { toast },
    '@/utils/async': load('../src/utils/async.ts'),
  })
  const { useAccountEditor } = load('../src/views/accounts/composables/useAccountEditor.ts', {
    vue,
    '@/api': { updateAccount: async payload => updates.push(plain(payload)) },
    '@/api/modules/ipv6-egress': { getIpv6Egress: async () => ({ accountOverrides: {} }) },
    '@/components/base/BaseToast': { toast },
    '@/composables/useAsyncAction': asyncAction,
    '../utils/schedulingForm': scheduling,
    '@/views/accounts/utils/schedulingForm': scheduling,
    '../utils/modelAccess': load('../src/views/accounts/utils/modelAccess.ts'),
  })
  const account = { id: 'acct_purchase', provider: 'openai', authenticationKind: 'oauth', enabled: true, concurrencyLimit: null, weight: 1, groups: [], purchaseCost: saved }
  const scope = vue.effectScope()
  t.after(() => scope.stop())
  const editor = scope.run(() => useAccountEditor({ accounts: vue.ref([account]), reloadAccounts: async () => {}, reloadGroups: async () => {} }))
  editor.open(account)
  assert.equal(editor.purchaseAmount.value, '50')
  assert.equal(editor.purchaseCycleStart.value, '2026-01-31')
  await editor.save()
  assert.equal('purchaseCost' in updates[0], false)
  editor.open(account)
  editor.purchaseAmount.value = '60'
  await editor.save()
  assert.equal(updates[1].purchaseCost.amountCny, '60')
  editor.open(account)
  editor.purchaseAmount.value = ''
  await editor.save()
  assert.equal(updates[2].purchaseCost.amountCny, null)
  editor.open(account)
  assert.equal(editor.purchaseAmount.value, '50')
  editor.purchaseAmount.value = '-1'
  await editor.save()
  assert.equal(updates.length, 3)
  assert.ok(errors.length > 0)
})
