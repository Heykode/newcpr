import type { ReloginEntry, ReloginPoolAccount, ReloginStatus } from '@/api/modules/relogin'

export const statusLabels: Record<ReloginStatus, string> = {
  pending: '待处理',
  queued: '排队中',
  running: '重登中',
  awaiting_workspace: '待选择工作区',
  ready: '待推送',
  pushing: '推送中',
  uncertain: '推送待核实',
  failed: '重登失败',
}

export const recoveryLabels: Record<string, string> = {
  waiting: '等待重登',
  cooldown: '重试冷却中',
  loop_guard: '频繁失效保护',
  retry_limit: '自动重试已停止',
  manual_required: '需人工处理',
  disabled: '自动重登已关闭',
  paused: '队列已暂停',
  account_disabled: '账号已暂停调度',
  workspace_required: '工作区待确认',
  invalid_material: '重登资料不完整',
  unsupported_error: '需人工检查',
}

export function processingStatus(row: ReloginEntry) {
  if (row.status === 'awaiting_workspace')
    return { key: row.status, label: statusLabels[row.status], tone: 'text-cp-warning', detail: row.message }
  const recovery = row.recovery
  if (!['running', 'pushing', 'uncertain', 'queued'].includes(row.status) && recovery && recoveryLabels[recovery.state]) {
    return {
      key: recovery.state,
      label: recoveryLabels[recovery.state],
      tone: recovery.state === 'waiting' ? 'text-cp-primary' : 'text-cp-warning',
      detail: [recovery.message, row.message].filter(Boolean).join('；'),
    }
  }
  if (row.status === 'ready' && row.poolStatus === 'synced')
    return { key: 'synced', label: '已同步到号池', tone: 'text-cp-success', detail: '本次凭据已推送；账号当前状态见号池列' }
  if (row.status === 'uncertain')
    return { key: row.status, label: statusLabels[row.status], tone: 'text-cp-warning', detail: '推送结果尚未确认，不能直接重复推送；请先核对号池记录' }
  return {
    key: row.status,
    label: statusLabels[row.status],
    tone: row.status === 'failed' ? 'text-cp-error' : ['running', 'pushing'].includes(row.status) ? 'text-cp-primary' : 'text-cp-text-secondary',
    detail: row.message,
  }
}

export function recoveryCountdown(row: ReloginEntry, now: number) {
  const retryAt = row.recovery?.retryAt
  if (!retryAt)
    return ''
  const remaining = Date.parse(retryAt) - now
  if (!Number.isFinite(remaining))
    return ''
  if (remaining <= 0)
    return '等待下一轮检查'
  const seconds = Math.ceil(remaining / 1000)
  return `剩余 ${Math.floor(seconds / 60)}分${String(seconds % 60).padStart(2, '0')}秒`
}

export function retryProgress(row: ReloginEntry) {
  const recovery = row.recovery
  if (['pushing', 'uncertain'].includes(row.status)
    || !recovery || !['cooldown', 'retry_limit', 'waiting', 'running', 'manual_required'].includes(recovery.state)
    || recovery.retriesUsed == null || recovery.maxRetries == null) {
    return ''
  }
  return `已重试 ${recovery.retriesUsed}/${recovery.maxRetries}`
}

export function credentialLabel(row: ReloginEntry) {
  return { none: '尚未获取', verified: '新凭据已验证', expired: '缓存凭据已过期' }[row.credentialStatus]
}

const errorLabels: Record<string, string> = {
  account_unverified: '账号未验证',
  access_token_expired: '访问令牌已过期',
  credential_expired: '凭据失效',
  credential_invalid: '凭据无效',
  account_banned: '账号被封禁',
}

function accountState(account: ReloginPoolAccount) {
  return account.status === 'error'
    ? errorLabels[account.errorReason ?? ''] ?? '账号异常'
    : { normal: '正常', rate_limited: '请求限流', quota_exhausted: '额度耗尽', disabled: '暂停调度' }[account.status]
}

function matchedPoolAccounts(row: ReloginEntry) {
  const accounts = row.poolAccounts ?? []
  const target = accounts.find(account => account.id === row.reloginAccountId)
  return target ? [target] : accounts
}

export function matchesPool(row: ReloginEntry, filter: string) {
  if (!filter)
    return true
  if (filter === 'absent')
    return !row.poolAccountIds.length
  if (filter === 'present')
    return row.poolAccountIds.length > 0
  return matchedPoolAccounts(row).some(account =>
    filter === 'disabled' ? !account.enabled : account.status === filter,
  )
}

export function poolPresentation(row: ReloginEntry) {
  if (!row.poolAccountIds.length)
    return { key: 'absent', label: '未入池', caption: '', tone: 'text-cp-text-tertiary', detail: '' }
  const accounts = matchedPoolAccounts(row)
  if (!accounts.length)
    return { key: 'unknown', label: '状态未知', caption: '已在池中', tone: 'text-cp-text-secondary', detail: '服务器尚未返回号池当前状态' }
  const detail = accounts.map(account => [
    account.workspaceId ?? account.id,
    accountState(account),
    !account.enabled ? '暂停调度' : '',
    account.errorMessage ?? '',
  ].filter(Boolean).join(' · ')).join('\n')
  if (accounts.length > 1) {
    const errors = accounts.filter(account => account.status !== 'normal').length
    const paused = accounts.filter(account => !account.enabled).length
    return {
      key: 'multiple',
      label: `${accounts.length} 个池中账号`,
      caption: errors || paused ? `${errors} 异常 · ${paused} 暂停` : '工作区待指定',
      tone: errors ? 'text-cp-error' : paused ? 'text-cp-warning' : 'text-cp-text-secondary',
      detail,
    }
  }
  const account = accounts[0]!
  return {
    key: !account.enabled ? 'disabled' : account.status,
    label: !account.enabled && account.status === 'normal' ? '暂停调度' : accountState(account),
    caption: !account.enabled && account.status !== 'normal' ? '暂停调度' : '已在池中',
    tone: account.status === 'error' ? 'text-cp-error' : !account.enabled || account.status !== 'normal' ? 'text-cp-warning' : 'text-cp-success',
    detail,
  }
}

export function workspaceChoices(row: ReloginEntry) {
  if (row.status === 'awaiting_workspace') {
    return (row.workspaceChoices ?? []).map(choice => ({
      value: choice.id,
      label: `${choice.name || '未命名工作区'} · ${choice.planType.toUpperCase()}`,
      description: choice.id,
    }))
  }
  const known = new Map<string, string | null>()
  for (const account of row.poolAccounts ?? []) {
    if (account.workspaceId)
      known.set(account.workspaceId, account.planType)
  }
  if (row.workspaceId)
    known.set(row.workspaceId, row.planType)
  return [...known].map(([value, plan]) => ({
    value,
    label: `${plan?.toUpperCase() ?? '工作区'} · ${value}`,
  }))
}

export function workspaceId(row: ReloginEntry) {
  return row.workspaceId ?? row.preferredWorkspaceId
    ?? row.poolAccounts?.find(account => account.id === row.reloginAccountId)?.workspaceId ?? null
}

export function shortWorkspace(id: string | null) {
  return id ? (id.length > 18 ? `${id.slice(0, 8)}…${id.slice(-6)}` : id) : '自动选择'
}
