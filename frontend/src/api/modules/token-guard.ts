import type { RequestOptions } from '../request'
import request from '../request'

export interface TokenGuardConfig {
  enabled: boolean
  groupIds: string[]
  model: string
  intervalSeconds: number
  timeoutSeconds: number
  concurrency: number
  maxPerCycle: number
}
export interface TokenGuardEvent {
  accountId: string
  observedRevision: number
  outcome: 'healthy' | 'auth_required' | 'transient' | 'skipped' | 'timed_out'
  reason: 'completed' | 'credential_expired' | 'credential_invalid' | 'account_banned' | 'account_disabled' | 'quota_exhausted' | 'account_changed' | 'request_failed' | 'timeout'
  latencyMs: number
  observedAt: string
}
export interface TokenGuardStatus {
  config: TokenGuardConfig
  running: boolean
  queued: boolean
  events: TokenGuardEvent[]
}
export function getTokenGuard(options: RequestOptions = {}) {
  return request<TokenGuardStatus>({ url: '/api/admin/token-guard', method: 'GET', ...options })
}
export function configureTokenGuard(data: TokenGuardConfig) {
  return request<void>({ url: '/api/admin/token-guard/config', method: 'POST', data })
}
export function runTokenGuard() {
  return request<void>({ url: '/api/admin/token-guard/run', method: 'POST' })
}
export function reloginTokenGuardAccount(accountId: string) {
  return request<void>({ url: '/api/admin/token-guard/relogin', method: 'POST', data: { accountId } })
}
