import type { RequestOptions } from '../request'
import type { AccountTemplateSelection } from './account-templates'
import request from '../request'

export type ReloginStatus = 'pending' | 'queued' | 'running' | 'awaiting_workspace' | 'ready' | 'pushing' | 'uncertain' | 'failed'
export interface ReloginWorkspaceChoice { id: string, name: string, planType: string }
export type ReloginWorkspaceMode = 'original' | 'highest'
export interface ReloginPushSelection { accountId: string, switchWorkspace: boolean }
export interface ReloginPushTarget extends ReloginPushSelection {
  workspaceId: string
  planType: string | null
  available: boolean
}
export interface ReloginPoolAccount {
  id: string
  workspaceId: string | null
  planType: string | null
  enabled: boolean
  status: 'normal' | 'error' | 'rate_limited' | 'quota_exhausted' | 'disabled'
  errorReason: string | null
  errorMessage: string | null
}
export interface ReloginEntry {
  id: string
  revision: number
  email: string
  hasTotp: boolean
  automatic: boolean
  status: ReloginStatus
  message: string
  recovery?: { state: string, message: string, retryAt: string | null, retriesUsed?: number, maxRetries?: number }
  planType: string | null
  workspaceId: string | null
  preferredWorkspaceId: string | null
  workspaceMode?: ReloginWorkspaceMode
  workspaceChoices?: ReloginWorkspaceChoice[]
  pushTargets?: ReloginPushTarget[]
  credentialStatus: 'none' | 'verified' | 'expired'
  poolStatus: 'absent' | 'present' | 'pending_push' | 'synced'
  poolAccountIds: string[]
  poolAccounts?: ReloginPoolAccount[]
  reloginAccountId: string | null
  reloginCount: number | null
  lastReloginAt: string | null
  verifiedAt: string | null
  expiresAt: string | null
  importedAt?: string | null
  updatedAt: string
}
export interface ReloginSettings { concurrency: number, paused: boolean, maxRetries: number, retryIntervalMinutes: number }
export type ReloginSettingsUpdate = Pick<ReloginSettings, 'concurrency' | 'paused'> & Partial<Pick<ReloginSettings, 'maxRetries' | 'retryIntervalMinutes'>>
export interface ReloginList { settings: ReloginSettings, items: ReloginEntry[] }
export interface ReloginBatchResult { id: string, success: boolean, message: string }

export interface ReloginTarget {
  account_id: string
  credential_revision: number
  user_id: string
  workspace_id: string
}
export interface AccountReloginAction {
  accountId: string
  entryId: string
  revision: number
  target: ReloginTarget | null
  status: ReloginStatus
  message: string
  busy: boolean
  blockedReason: string | null
  syncedAt: string | null
}
export function getAccountReloginActions(ids: string[], options: RequestOptions = {}) {
  return request<AccountReloginAction[]>({ url: '/api/admin/relogin/accounts/query', method: 'POST', data: { ids }, ...options })
}
export function queueAccountRelogin(data: { entryId: string, revision: number, target: ReloginTarget }) {
  return request<void>({ url: '/api/admin/relogin/accounts/queue', method: 'POST', data })
}

export function getRelogin(options: RequestOptions = {}) {
  return request<ReloginList>({ url: '/api/admin/relogin', method: 'GET', ...options })
}
export function importRelogin(text: string, replaceExisting: boolean) {
  return request<{ imported: number }>({ url: '/api/admin/relogin/import', method: 'POST', data: { text, replaceExisting } })
}
export function queueRelogin(ids: string[], workspaceMode: ReloginWorkspaceMode = 'original') {
  return request<ReloginBatchResult[]>({ url: '/api/admin/relogin/queue', method: 'POST', data: { ids, workspaceMode } })
}
export function pushRelogin(rows: Pick<ReloginEntry, 'id' | 'revision'>[], template?: AccountTemplateSelection, customName?: string, selections?: Record<string, ReloginPushSelection>, newAccountExcel?: import('@/utils/excel-settings').ExcelSettings) {
  const ids = rows.map(row => row.id)
  const revisions = Object.fromEntries(rows.map(row => [row.id, row.revision]))
  return request<ReloginBatchResult[]>({ url: '/api/admin/relogin/push', method: 'POST', data: { ids, revisions, ...(template ? { template } : {}), ...(customName ? { customName } : {}), ...(selections ? { selections } : {}), ...(newAccountExcel ? { newAccountExcel } : {}) }, timeout: 120000 })
}
export function deleteRelogin(ids: string[]) {
  return request<void>({ url: '/api/admin/relogin/delete', method: 'POST', data: { ids } })
}
export function setReloginAutomatic(ids: string[], enabled: boolean) {
  return request<void>({ url: '/api/admin/relogin/automatic', method: 'POST', data: { ids, enabled } })
}
export function setReloginWorkspace(id: string, workspaceId: string | null) {
  return request<void>({ url: '/api/admin/relogin/workspace', method: 'POST', data: { id, workspaceId } })
}
export function resumeReloginWorkspace(id: string, revision: number, workspaceId: string) {
  return request<void>({ url: '/api/admin/relogin/workspace/resume', method: 'POST', data: { id, revision, workspaceId } })
}
export function configureRelogin(data: ReloginSettingsUpdate) {
  return request<void>({ url: '/api/admin/relogin/settings', method: 'POST', data })
}
