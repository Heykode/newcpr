import type { RequestOptions } from '../request'
import request from '../request'

export type MonitorStatus = 'ready' | 'partial' | 'learning' | 'unknown' | 'disabled' | 'idle' | 'empty'

export interface GroupMonitorItem {
  activeAlerts?: string[]
  id: string
  name: string
  color: string
  enabled: boolean
  totalAccounts: number
  eligibleAccounts: number
  estimatedAccounts: number
  usedSlots: number | null
  totalSlots: number
  remainingUsd: number | null
  remainingStatus: MonitorStatus
  expectedExpiryUsd: number | null
  expiryStatus: MonitorStatus
  consumeUsdPerMinute: number | null
  quotaConsumeUsdPerMinute: number | null
  etaMinutes: number | null
  etaStatus: MonitorStatus
  lowSample: boolean
  earliestResetAt: string | null
}

export interface GroupMonitorResponse {
  viewerScope: string
  generatedAt: string
  rateWindowSeconds: number
  items: GroupMonitorItem[]
}

export function getGroupMonitor(groupIds: string[], options: RequestOptions & { refreshForecasts?: boolean } = {}) {
  const { refreshForecasts, ...requestOptions } = options
  return request<GroupMonitorResponse>({
    url: '/api/admin/account-groups/monitor',
    method: 'GET',
    params: { groupIds: groupIds.join(','), ...(refreshForecasts ? { refreshForecasts: true } : {}) },
    ...requestOptions,
  })
}
