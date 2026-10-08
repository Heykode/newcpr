import type { RequestOptions } from '../request'
import type { AccountResetCredit, AccountResetCreditsResponse } from './accounts'
import request from '../request'

export interface ResetInventory {
  accountId: string
  checkedAt: string
  credits: AccountResetCreditsResponse | null
  error: string | null
  pending: { redeemRequestId: string, creditId: string | null, retryAfter: string } | null
}
export type ResetItemStatus = 'ready' | 'queued' | 'running' | 'succeeded' | 'skipped' | 'failed' | 'unknown'
export interface ResetBatchItem {
  accountId: string
  availableCount: number | null
  credit: AccountResetCredit | null
  redeemRequestId: string
  status: ResetItemStatus
  message: string
  updatedAt: string
  retry: boolean
}
export interface ResetBatch {
  id: string
  createdAt: string
  confirmed: boolean
  resetType: string | null
  items: ResetBatchItem[]
}
const base = '/api/admin/accounts/reset-credits'
export interface AutoResetConfig {
  enabled: boolean
  fiveHourUsedMillis: number
  sevenDayUsedMillis: number
}
export interface AutoResetPolicy {
  accountId: string
  revision: number
  config: AutoResetConfig
  checkedAt: string | null
  message: string | null
}
export function getAutoResetPolicy(accountId: string, options: RequestOptions = {}) {
  return request<AutoResetPolicy>({ url: `${base}/automatic`, method: 'GET', params: { accountId }, ...options })
}
export function saveAutoResetPolicy(accountId: string, revision: number, config: AutoResetConfig) {
  return request<AutoResetPolicy>({ url: `${base}/automatic`, method: 'POST', data: { accountId, revision, config }, silent: true })
}
export function getResetInventory(accountIds: string[], options: RequestOptions = {}) {
  return request<ResetInventory[]>({ url: `${base}/cache`, method: 'POST', data: { accountIds }, ...options })
}
export function refreshResetInventory(accountIds: string[], options: RequestOptions = {}) {
  return request<ResetInventory[]>({ url: `${base}/refresh`, method: 'POST', data: { accountIds }, timeout: 0, ...options })
}
export function previewResetBatch(accountIds: string[], resetType?: string, options: RequestOptions = {}) {
  return request<ResetBatch>({ url: `${base}/preview`, method: 'POST', data: { accountIds, resetType }, timeout: 0, ...options })
}
export function confirmResetBatch(id: string) {
  return request<ResetBatch>({ url: `${base}/confirm`, method: 'POST', data: { id }, silent: true })
}
export function getResetBatches(options: RequestOptions = {}) {
  return request<ResetBatch[]>({ url: `${base}/batches`, method: 'GET', ...options })
}
export interface ResetHistoryPage {
  items: ResetBatch[]
  accountNames: Record<string, string>
  before: string
  hasMore: boolean
}
export function getResetHistory(params: { page: number, search: string, before?: string }, options: RequestOptions = {}) {
  return request<ResetHistoryPage>({ url: `${base}/history`, method: 'GET', params, ...options })
}
export function retryResetBatch(id: string, accountId: string) {
  return request<void>({ url: `${base}/retry`, method: 'POST', data: { id, accountId }, silent: true })
}
