import type { Account } from '@/api'

export function turnStateBlockReason(account: Partial<Pick<Account, 'enabled' | 'status' | 'errorReason'>>): string | null {
  if (account.enabled === false || account.status === 'disabled')
    return '账号已暂停，采集已停止'
  if (account.status === 'error') {
    switch (account.errorReason) {
      case 'access_token_expired':
        return '凭据已过期，等待自动刷新'
      case 'credential_expired':
        return '凭据已失效，需要重新登录'
      case 'account_unverified':
        return '账号需要完成身份验证'
      case 'account_banned':
        return '账号已封禁，采集已停止'
      default:
        return '凭据异常，采集已停止'
    }
  }
  if (account.status === 'quota_exhausted')
    return '额度已耗尽，等待恢复'
  if (account.status === 'rate_limited')
    return '限流冷却中，采集已暂停'
  return null
}

export function turnStateProbeReason(reason?: string | null): string | null {
  if (!reason)
    return null
  const labels: Record<string, string> = {
    invalid_envelope: 'State格式不合格',
    unexpected_shape: 'State类型不合格',
    future_state: 'State时间超前',
    insufficient_lifetime: 'State剩余时间不足',
    duplicate_or_write_conflict: '重复值或写入冲突',
    missing_state: '响应未携带State',
    missing_completed: '响应未明确完成',
    stream_failed: '上游响应失败',
    stream_error: '响应流中断',
    timeout: '单次请求超时',
    transport_error: '传输失败',
    upstream_5xx: '上游服务错误',
    egress_unavailable: '无可用IPv6出口',
    account_rejected: '账号已被上游拒绝',
    account_stopped: '账号采集已停止',
    rate_limited: '账号限流冷却',
  }
  return labels[reason] ?? (/^upstream_\d{3}$/.test(reason) ? `上游HTTP ${reason.slice(9)}` : '探测失败')
}
