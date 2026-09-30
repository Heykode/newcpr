import type { AccountPurchaseCost, AccountPurchaseUpdate } from '@/api/modules/accounts'

export function chinaToday(now = new Date()): string {
  return new Date(now.getTime() + 8 * 3600 * 1000).toISOString().slice(0, 10)
}

export function purchaseCostPatch(amount: string, date: string): AccountPurchaseUpdate {
  const value = amount.trim()
  if (value && !/^\d{1,10}(?:\.\d{1,10})?$/.test(value))
    throw new Error('月成本必须为非负金额，最多10位整数和10位小数')
  const parsed = date ? new Date(`${date}T00:00:00Z`) : null
  if (date && (!/^\d{4}-\d{2}-\d{2}$/.test(date) || date < '2000-01-01' || date > chinaToday()
    || !parsed || Number.isNaN(parsed.getTime()) || parsed.toISOString().slice(0, 10) !== date)) {
    throw new Error('请选择2000年起至今天之间的有效起算日期')
  }
  return { amountCny: value || null, ...(date ? { cycleStart: date } : {}) }
}

export function purchaseCostLabel(cost?: AccountPurchaseCost | null): string {
  if (!cost || cost.amountCny === null || cost.breakevenCnyPerUsd === null)
    return '—'
  // Calculations stay decimal on the server; Number is display-only.
  const amount = Number(cost.breakevenCnyPerUsd)
  if (!Number.isFinite(amount) || amount < 0)
    return '—'
  if (amount > 0 && amount < 0.000001)
    return '<0.000001'
  return new Intl.NumberFormat('zh-CN', { maximumFractionDigits: 6 }).format(amount)
}

export function purchaseCostTitle(cost?: AccountPurchaseCost | null): string {
  if (!cost)
    return '尚无可关联的成本身份'
  const parts = [
    `每账号月成本：${cost.amountCny === null ? '未设置' : `${trimAmount(cost.amountCny)} 元`}`,
    `本期累计消费：${trimAmount(cost.usageUsd)} 美元`,
    ...(cost.periodStart && cost.periodEnd ? [`周期：${cost.periodStart} 至 ${cost.periodEnd}（不含结束日，上海时间）`] : []),
    '保本价 = 本期人民币成本 ÷ 本期美元消费，不换汇',
  ]
  if (!cost.historyComplete)
    parts.push('历史记录不完整，仅统计可核实消费')
  return parts.join('\n')
}

export function trimAmount(value: string): string {
  return value.includes('.') ? value.replace(/0+$/, '').replace(/\.$/, '') : value
}
