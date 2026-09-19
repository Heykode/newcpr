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
  return null
}
