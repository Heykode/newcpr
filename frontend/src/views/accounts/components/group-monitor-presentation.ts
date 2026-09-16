import type { AccountGroup, MonitorStatus } from '@/api'

export function readPinnedGroups(raw: unknown): string[] {
  if (!Array.isArray(raw))
    return []
  return [...new Set(raw.filter((id): id is string => typeof id === 'string' && /^grp_[\w-]{1,128}$/.test(id)))].slice(0, 200)
}

export function orderMonitorGroups(groups: AccountGroup[], pins: string[]) {
  const rank = new Map(pins.map((id, index) => [id, index]))
  return groups.filter(group => rank.has(group.id) || group.memberCount > 0).sort((left, right) =>
    (rank.get(left.id) ?? Number.MAX_SAFE_INTEGER) - (rank.get(right.id) ?? Number.MAX_SAFE_INTEGER)
    || left.createdAt.localeCompare(right.createdAt)
    || left.id.localeCompare(right.id))
}

export function monitorMoney(value: number | null | undefined, status: MonitorStatus = 'unknown', digits = 2) {
  if (status === 'disabled')
    return '已停用'
  if (value != null && Number.isFinite(value) && value >= 0)
    return `$${value.toLocaleString('en-US', { minimumFractionDigits: digits, maximumFractionDigits: digits })}`
  return status === 'learning' ? '暂无估值' : '未知'
}

export function monitorEta(minutes: number | null | undefined, status: MonitorStatus) {
  if (status === 'disabled')
    return '已停用'
  if (status === 'idle')
    return '暂无消耗'
  if (minutes == null || !Number.isFinite(minutes) || minutes < 0)
    return status === 'learning' || status === 'partial' ? '待估算' : '未知'
  if (minutes === 0)
    return '0 分钟'
  if (minutes < 1)
    return '<1 分钟'
  if (minutes < 60)
    return `${Math.floor(minutes)} 分钟`
  const hours = Math.floor(minutes / 60)
  if (hours < 24)
    return `${hours}h ${Math.floor(minutes % 60)}m`
  return `${Math.floor(hours / 24)}d ${hours % 24}h`
}
