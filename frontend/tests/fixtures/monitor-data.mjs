export const groups = [
  ['常用 Plus', '#16A34AFF'],
  ['高并发 Pro', '#2563EBFF'],
  ['长对话测试组与备用账号容量观测', '#CA8A04FF'],
  ['备用账号', '#0891B2FF'],
  ['停用分组', '#737373FF'],
  ['空分组', '#D946EFFF'],
].map(([name, color], index) => ({
  id: `grp_demo${index + 1}`,
  name,
  color,
  enabled: index !== 4,
  description: null,
  memberCount: index === 5 ? 0 : 40,
  providerCounts: { openai: 40 },
  clientKeyCount: 1,
  accountSummary: { available: 40, limited: 0, total: 40 },
  capacity: { usedSlots: 7, totalSlots: 120 },
  usage: { todayUsd: '50', retainedTotalUsd: '100' },
  createdAt: `2026-01-0${index + 1}T00:00:00Z`,
  updatedAt: '2026-01-10T00:00:00Z',
}))

export function monitorResponse(ids) {
  return {
    viewerScope: 'admin:local-fixture',
    generatedAt: new Date().toISOString(),
    rateWindowSeconds: 60,
    items: groups.filter(group => ids.includes(group.id)).map((group, index) => ({
      ...group,
      totalAccounts: group.memberCount,
      eligibleAccounts: group.enabled && group.memberCount ? 38 : 0,
      estimatedAccounts: group.enabled && group.memberCount ? 38 : 0,
      usedSlots: group.enabled && group.memberCount ? 7 : 0,
      totalSlots: group.enabled && group.memberCount ? 114 : 0,
      remainingUsd: group.enabled ? group.memberCount ? 1280.45 + index * 130 : 0 : null,
      remainingStatus: group.enabled ? 'ready' : 'disabled',
      expectedExpiryUsd: !group.memberCount ? 0 : group.id === 'grp_demo3' ? null : 82.1,
      expiryStatus: !group.enabled ? 'disabled' : group.id === 'grp_demo3' ? 'learning' : 'ready',
      consumeUsdPerMinute: group.memberCount ? 0.125 : 0,
      quotaConsumeUsdPerMinute: group.memberCount ? 0.2 : 0,
      etaMinutes: group.enabled ? group.memberCount ? 6402 : 0 : null,
      etaStatus: group.enabled ? group.memberCount ? 'ready' : 'empty' : 'disabled',
      lowSample: false,
      earliestResetAt: new Date(Date.now() + 3_600_000).toISOString(),
    })),
  }
}
