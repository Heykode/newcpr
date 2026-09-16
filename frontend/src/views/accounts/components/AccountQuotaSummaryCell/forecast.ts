import type { AccountUsage } from '@/api'

export function weeklyForecastPresentation(usage: AccountUsage, now: number) {
  const source = usage.quotaWindow
  if (source?.period !== 'weekly')
    return { amount: '—', title: '暂无本轮 7D 消费窗口；点击查看预测详情' }
  const resetAt = Date.parse(source.resetAt ?? '')
  if (!Number.isFinite(resetAt) || resetAt <= now)
    return { amount: '—', title: '缺少有效的额度窗口或窗口已过期；点击查看预测详情' }
  const estimate = source.estimatedUsd
  if (typeof estimate !== 'number' || !Number.isFinite(estimate) || estimate <= 0)
    return { amount: '—', title: '本轮 7D 消费或已用比例不足；点击查看预测详情' }
  const notes = ['本轮 7D 消费 ÷ 已用比例，不是官方余额']
  if (usage.costEstimateStatus === 'partial')
    notes.push('费用记录不完整，结果可能偏低')
  return {
    amount: `$${estimate.toFixed(2)}`,
    title: notes.join('；'),
  }
}
