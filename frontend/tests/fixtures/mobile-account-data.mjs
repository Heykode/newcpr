import { accounts as base } from './relogin-count-data.mjs'

const resetAt = new Date(Date.now() + 3 * 3600_000).toISOString()
export const mobileAccounts = Array.from({ length: 6 }, (_, index) => {
  const row = structuredClone(base[index % base.length])
  const percent = [24, 91, 62, 0, null, 8][index]
  return {
    ...row,
    id: `acct_mobile_${index}`,
    customName: ['研发主账号', '团队备用账号', '等待凭据恢复', null, '尚未获取配额', '已暂停账号'][index],
    email: index === 0 ? 'long.account.name.for.mobile.layout@example.invalid' : `mobile-${index}@example.invalid`,
    enabled: index !== 5,
    status: index === 2 ? 'error' : index === 1 ? 'rate_limited' : 'normal',
    errorReason: index === 2 ? 'credential_expired' : null,
    errorMessage: index === 2 ? 'Synthetic credential expiry' : null,
    responsesUpstream: index === 0 ? 'excel' : 'codex',
    qualityMonitoring: index === 0
      ? {
          ruleId: 'rule-mobile-fixture',
          revision: 1,
          enabled: true,
          running: false,
          pending: false,
          nextRunAt: resetAt,
          lastStatus: 'healthy',
          lastRunAt: null,
          lastAction: null,
        }
      : null,
    inFlight: index === 4 ? null : index,
    effectiveConcurrencyLimit: index === 4 ? null : 10,
    healthTimeline: index === 4
      ? []
      : Array.from({ length: 6 }, (_, bucketIndex) => ({
          key: `health-${bucketIndex}`,
          startAt: new Date(Date.now() - (6 - bucketIndex) * 300_000).toISOString(),
          requestCount: 10,
          successCount: [10, 9, 6, 0, 10, 10][bucketIndex],
          errorCount: [0, 1, 3, 0, 0, 0][bucketIndex],
          nonCompletionCount: bucketIndex === 2 ? 1 : 0,
          inFlightCount: bucketIndex === 3 ? 2 : 0,
        })),
    groups: index === 0
      ? [
          { id: 'group-fixture-1', name: '研发', color: '#18a058', enabled: true },
          { id: 'group-fixture-2', name: '移动端长分组名称换行验证', color: '#1677ff', enabled: true },
        ]
      : [],
    quota: {
      ...row.quota,
      rateLimitedUntil: index === 1 ? resetAt : null,
      credits: { hasCredits: true, unlimited: false, balance: '123.45' },
      windows: [18000, 604800].map((windowSeconds, windowIndex) => ({
        key: `codex-${windowIndex}`,
        group: 'shortTerm',
        limitId: 'codex',
        limitName: 'Codex',
        role: windowIndex ? 'secondary' : 'primary',
        windowSeconds,
        labelDisplay: windowIndex ? '7 天额度' : '5 小时额度',
        windowLabelDisplay: windowIndex ? '7 天' : '5 小时',
        usedPercent: percent,
        usedPercentDisplay: percent === null ? '未知' : `${percent}%`,
        limitReached: false,
        resetAt,
        resetAtDisplay: '稍后重置',
      })),
    },
    purchaseCost: {
      amountCny: '50',
      cycleAnchor: '2026-01-01',
      periodStart: '2026-01-01',
      periodEnd: '2026-02-01',
      usageUsd: '100',
      breakevenCnyPerUsd: '0.5',
      historyComplete: true,
      historyCompleteFrom: null,
    },
    usage: {
      ...row.usage,
      requestCount: 12,
      totalTokens: 18000,
      totalTokensDisplay: '18K',
      windowLabelDisplay: '近 24 小时',
      inputTokensDisplay: '12K',
      outputTokensDisplay: '6K',
      cachedTokensDisplay: '9K',
      reasoningTokensDisplay: '2K',
      readTokensDisplay: '9K',
      costs: [{ currency: 'USD', estimatedAmount: '12.50', estimatedAmountDisplay: '$12.50' }],
    },
  }
})
