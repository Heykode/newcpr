import type { AccountErrorReason, AccountStatus, getAccounts } from '@/api'
import { defineTableColumns } from '@/components/base/BaseTable/columns'

export type AccountRow = Awaited<ReturnType<typeof getAccounts>>['items'][number]
export type AccountQuotaWindow = AccountRow['quota']['windows'][number]

export interface AccountQuotaWindowEntry {
  key: string
  label: string | null
  windows: AccountQuotaWindow[]
}

const quotaGroupOrder = new Map([
  ['shortTerm', 0],
  ['monthly', 1],
  ['other', 2],
])

export const accountColumns = defineTableColumns<AccountRow>([
  { key: 'expander', kind: 'expander' },
  { key: 'selection', kind: 'selection' },
  {
    key: 'identity',
    label: '账号',
    kind: 'identity',
    size: '3xl',
    grow: 1,
    sortable: 'email',
  },
  { key: 'status', label: '状态', kind: 'status', align: 'center', sortable: true },
  { key: 'planType', label: '套餐', kind: 'status', align: 'center', sortable: true },
  { key: 'capacity', label: '容量', kind: 'numeric', align: 'center' },
  { key: 'health', label: '健康', kind: 'custom', size: 'lg' },
  { key: 'usage', label: '用量', kind: 'custom', size: '2xl', sortable: true, grow: 1 },
  { key: 'groups', label: '账号分组', kind: 'status', size: 'xl' },
  {
    key: 'lastUsedAt',
    label: '最后使用',
    kind: 'datetime',
    size: 'md',
    sortable: true,
    emptyText: '',
  },
  {
    key: 'reloginCount',
    label: '重登次数',
    kind: 'numeric',
    size: 'md',
    align: 'center',
    sortable: true,
  },
  {
    key: 'addedAt',
    label: '创建时间',
    kind: 'datetime',
    sortable: true,
    format: (_value, row) => row.addedAtDisplay,
  },
  {
    key: 'accessTokenExpiresAtDisplay',
    label: '令牌过期',
    kind: 'datetime',
    sortable: 'expiresAt',
    format: value => optionalAccountCell(value),
    emptyText: '',
  },
  { key: 'actions', label: '操作', kind: 'actions', size: 'lg' },
])

export const accountColumnOptions = accountColumns
  .filter(column => !['expander', 'selection', 'actions'].includes(column.key))
  .map(column => ({ key: column.key, label: column.label ?? column.key }))

export function readAccountColumnKeys(raw: string): string[] {
  const defaults = accountColumnOptions.map(column => column.key)
  try {
    const stored: unknown = JSON.parse(raw)
    const legacy = Array.isArray(stored)
    const keys: unknown = legacy
      ? stored
      : stored && typeof stored === 'object' && 'version' in stored && stored.version === 2 && 'keys' in stored
        ? stored.keys
        : null
    if (!Array.isArray(keys))
      return defaults
    const selected = [...new Set(keys
      .map(key => key === 'addedAtDisplay' ? 'addedAt' : key)
      .filter((key): key is string => typeof key === 'string' && defaults.includes(key)))]
    if (legacy && !selected.includes('reloginCount'))
      selected.push('reloginCount')
    return selected
  }
  catch {
    return defaults
  }
}

export function writeAccountColumnKeys(keys: string[]): string {
  return JSON.stringify({ version: 2, keys })
}

export const statusLabels: Record<AccountStatus, string> = {
  normal: '正常',
  quota_exhausted: '配额耗尽',
  rate_limited: '限流中',
  disabled: '暂停',
  error: '错误',
}

export const statusTones: Record<AccountStatus, 'success' | 'danger' | 'warning' | 'info' | 'normal'> = {
  normal: 'success',
  quota_exhausted: 'warning',
  rate_limited: 'warning',
  disabled: 'normal',
  error: 'danger',
}

export const accountStatusFilterOptions = [
  { label: '全部状态', value: '' },
  { label: statusLabels.normal, value: 'normal' },
  { label: statusLabels.quota_exhausted, value: 'quota_exhausted' },
  { label: statusLabels.rate_limited, value: 'rate_limited' },
  { label: statusLabels.disabled, value: 'disabled' },
  { label: statusLabels.error, value: 'error' },
]

/** `error` 分类下具体原因的展示文案（对应后端 `errorReason`）。 */
export const errorReasonLabels: Record<AccountErrorReason, string> = {
  account_unverified: '账号身份尚未确认',
  access_token_expired: 'Access Token 已过期',
  credential_expired: '凭据已过期',
  credential_invalid: '凭据无效',
  account_banned: '账号不可用或已被封禁',
}

/**
 * 后端统一派生状态，关闭开关优先返回 disabled；前端不再独立派生。
 */
export function derivedAccountStatus(row: AccountRow): AccountStatus {
  return row.status
}

export function visibleSummaryQuotaWindows(windows: AccountQuotaWindow[]) {
  const known = [...windows]
    .filter(window => window.group !== 'other')
    .sort(compareQuotaWindows)
  return known.length > 0 ? known : [...windows].sort(compareQuotaWindows)
}

export function orderedPanelQuotaWindows(windows: AccountQuotaWindow[]) {
  return [...windows].sort(compareQuotaWindows)
}

export function groupedAccountQuotaWindows(windows: AccountQuotaWindow[]) {
  const entries: AccountQuotaWindowEntry[] = []
  const limitEntryIndexes = new Map<string, number>()

  for (const window of windows) {
    const limitLabel = quotaLimitLabel(window)
    if (!window.limitId || !limitLabel) {
      entries.push({ key: window.key, label: null, windows: [window] })
      continue
    }

    const existingIndex = limitEntryIndexes.get(window.limitId)
    if (existingIndex !== undefined) {
      entries[existingIndex]?.windows.push(window)
      continue
    }

    limitEntryIndexes.set(window.limitId, entries.length)
    entries.push({
      key: `limit:${window.limitId}`,
      label: limitLabel,
      windows: [window],
    })
  }

  for (const entry of entries)
    entry.windows.sort(compareQuotaLimitWindows)

  return entries
}

export function modelSuccessRateTextClass(successRate: number | null) {
  if (successRate === null)
    return 'text-cp-text-quaternary'
  if (successRate >= 99.5)
    return 'text-cp-success-text'
  if (successRate >= 98)
    return 'text-cp-cyan-text'
  if (successRate >= 95)
    return 'text-cp-warning-text'
  return 'text-cp-error-text'
}

function compareQuotaWindows(left: AccountQuotaWindow, right: AccountQuotaWindow) {
  const groupDifference = groupOrder(left) - groupOrder(right)
  if (groupDifference !== 0)
    return groupDifference

  // 同组保留 Provider 的投影顺序，避免按窗口时长打散 core 与模型专属额度。
  return 0
}

function groupOrder(window: AccountQuotaWindow) {
  return quotaGroupOrder.get(window.group) ?? quotaGroupOrder.size
}

function compareQuotaLimitWindows(left: AccountQuotaWindow, right: AccountQuotaWindow) {
  const durationDifference = windowDurationOrder(left.windowSeconds)
    - windowDurationOrder(right.windowSeconds)
  if (durationDifference !== 0)
    return durationDifference

  return windowRoleOrder(left.role) - windowRoleOrder(right.role)
}

function windowDurationOrder(windowSeconds: number | null) {
  return windowSeconds ?? Number.POSITIVE_INFINITY
}

function windowRoleOrder(role: AccountQuotaWindow['role']) {
  switch (role) {
    case 'primary':
      return 0
    case 'secondary':
      return 1
    case 'monthly':
      return 2
    default:
      return 3
  }
}

function quotaLimitLabel(window: AccountQuotaWindow) {
  if (window.limitId === 'codex')
    return '通用额度'
  return window.limitName
}

function optionalAccountCell(value: unknown) {
  return value === '—' || value === '-' ? '' : value
}
