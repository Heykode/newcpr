import type { RequestOptions } from '../request'
import request from '../request'

export type ReloginStatus = 'pending' | 'queued' | 'running' | 'ready' | 'pushing' | 'uncertain' | 'failed'
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
  planType: string | null
  workspaceId: string | null
  preferredWorkspaceId: string | null
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
export interface ReloginSettings { concurrency: number, paused: boolean }
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
export function queueRelogin(ids: string[]) {
  return request<ReloginBatchResult[]>({ url: '/api/admin/relogin/queue', method: 'POST', data: { ids } })
}
export function pushRelogin(rows: Pick<ReloginEntry, 'id' | 'revision'>[], template?: ReloginTemplateSelection) {
  const ids = rows.map(row => row.id)
  const revisions = Object.fromEntries(rows.map(row => [row.id, row.revision]))
  return request<ReloginBatchResult[]>({ url: '/api/admin/relogin/push', method: 'POST', data: { ids, revisions, ...(template ? { template } : {}) }, timeout: 120000 })
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
export function configureRelogin(data: ReloginSettings) {
  return request<void>({ url: '/api/admin/relogin/settings', method: 'POST', data })
}

export interface ReloginTemplateConfig {
  name: string
  enabled: boolean
  concurrencyLimit: number | null
  weight: number
  groupIds: string[]
  outboundProxyId: string | null
}
export interface ReloginTemplateSelection { id: string, revision: number }
export interface ReloginTemplate extends ReloginTemplateSelection { config: ReloginTemplateConfig }
export function getReloginTemplates(options: RequestOptions = {}) {
  return request<ReloginTemplate[]>({ url: '/api/admin/relogin/templates', method: 'GET', ...options })
}
export function saveReloginTemplate(config: ReloginTemplateConfig, selection?: ReloginTemplateSelection) {
  return request<ReloginTemplate>({ url: '/api/admin/relogin/templates/save', method: 'POST', data: { config, selection } })
}
export function deleteReloginTemplate(selection: ReloginTemplateSelection) {
  return request<void>({ url: '/api/admin/relogin/templates/delete', method: 'POST', data: selection })
}
