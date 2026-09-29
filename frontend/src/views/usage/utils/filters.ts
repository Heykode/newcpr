import type { UsageFilterParams } from '@/api'

export const usageFilterKeys = [
  'accountId',
  'accountIds',
  'accountSearch',
  'clientApiKeyId',
  'groupId',
  'requestedModel',
  'upstreamModel',
  'upstreamMode',
  'clientTransport',
  'upstreamTransport',
  'clientStatusCode',
  'upstreamStatusCode',
  'requestId',
  'responseId',
  'upstreamRequestId',
  'clientIp',
  'minLatencyMs',
  'maxLatencyMs',
  'minFirstTokenMs',
  'maxFirstTokenMs',
  'cacheMatch',
  'failureKind',
  'errorCode',
  'errorPhase',
  'recovery',
  'errorScope',
  'route',
  'search',
  'startTime',
  'endTime',
  'view',
  'provider',
  'timeRange',
] as const

const numericKeys = new Set<string>([
  'clientStatusCode',
  'upstreamStatusCode',
  'minLatencyMs',
  'maxLatencyMs',
  'minFirstTokenMs',
  'maxFirstTokenMs',
])
const errorKeys = new Set<string>(['failureKind', 'errorCode', 'errorPhase', 'recovery', 'errorScope'])
const viewKeys = new Set<string>(['view', 'timeRange'])
export type UsageFilterDraft = Partial<Record<typeof usageFilterKeys[number], string>>

export function readUsageFilterDraft(query: Record<string, unknown>): UsageFilterDraft {
  return Object.fromEntries(usageFilterKeys.flatMap((key) => {
    const value = query[key]
    return typeof value === 'string' && value.trim()
      ? [[key, key === 'search' && value.trim().startsWith('sk_') ? value.trim().slice(0, 10) : value.trim()]]
      : []
  }))
}

export function usageFilterParams(draft: UsageFilterDraft, errors = false): UsageFilterParams {
  return Object.fromEntries(usageFilterKeys.flatMap((key) => {
    if (viewKeys.has(key) || (!errors && errorKeys.has(key)))
      return []
    let value = draft[key]?.trim()
    if (!value)
      return []
    if (key === 'search' && value.startsWith('sk_'))
      value = value.slice(0, 10)
    return [[key, numericKeys.has(key) ? Number(value) : value]]
  }))
}

export function usageFilterError(draft: UsageFilterDraft) {
  const enums: Partial<Record<keyof UsageFilterDraft, string[]>> = {
    upstreamMode: ['excel', 'codex', 'unknown'],
    clientTransport: ['http', 'http_sse', 'websocket'],
    upstreamTransport: ['http', 'http_sse', 'websocket', 'unknown'],
    cacheMatch: ['hit', 'miss', 'unknown'],
    recovery: ['recovered', 'unrecovered'],
    errorScope: ['requests', 'events', 'all'],
    view: ['success', 'errors'],
    timeRange: ['today', '7d', '30d'],
  }
  for (const [key, allowed] of Object.entries(enums)) {
    const value = draft[key as keyof UsageFilterDraft]
    if (value && !allowed.includes(value))
      return '筛选选项无效，请重新选择'
  }
  if (draft.accountIds && draft.accountIds.split(',').filter(Boolean).length > 50)
    return '最多选择 50 个账号'
  for (const key of usageFilterKeys) {
    // Response IDs retain the existing opaque, potentially long ID contract.
    if (key !== 'accountIds' && key !== 'responseId' && (draft[key]?.length || 0) > 256)
      return '筛选文本不能超过 256 个字符'
  }
  for (const key of numericKeys) {
    const value = draft[key as keyof UsageFilterDraft]?.trim()
    if (value && (!/^\d+$/.test(value) || !Number.isSafeInteger(Number(value))))
      return '状态码和耗时必须是非负整数'
    if (value && key.endsWith('StatusCode') && (Number(value) < 100 || Number(value) > 599))
      return 'HTTP 状态码必须在 100 到 599 之间'
  }
  for (const [min, max] of [['minLatencyMs', 'maxLatencyMs'], ['minFirstTokenMs', 'maxFirstTokenMs']] as const) {
    if (draft[min] && draft[max] && Number(draft[min]) > Number(draft[max]))
      return '耗时下限不能大于上限'
  }
  if ((draft.startTime && !draft.endTime) || (!draft.startTime && draft.endTime))
    return '请选择完整的起止时间'
  if (draft.startTime && draft.endTime
    && (!Number.isFinite(Date.parse(draft.startTime)) || !Number.isFinite(Date.parse(draft.endTime))
      || Date.parse(draft.startTime) >= Date.parse(draft.endTime))) {
    return '结束时间必须晚于开始时间'
  }
  return ''
}
