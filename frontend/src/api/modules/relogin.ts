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
  planTypeDisplay?: string | null
  available: boolean
}
export interface ReloginPoolAccount {
  id: string
  workspaceId: string | null
  planType: string | null
  planTypeDisplay?: string | null
  enabled: boolean
  status: 'normal' | 'error' | 'rate_limited' | 'quota_exhausted' | 'disabled'
  errorReason: string | null
  errorMessage: string | null
}
export interface ReloginEgress {
  kind: 'default' | 'proxy' | 'ipv6' | 'unavailable' | 'multiple'
  label: string
  source: 'account' | 'import' | 'default' | 'credential'
  accountId: string | null
  proxyId: string | null
  address: string | null
  exitIp: string | null
  checkedAt: string | null
  latencyMs?: number | null
}
export interface ReloginEntry {
  credentialEgress?: ReloginEgress
  enrollmentPlan?: ReloginEnrollmentPlan | null
  egress?: ReloginEgress
  lastActivity?: ReloginAttempt | null
  maintenance?: MaintenanceView
  id: string
  revision: number
  email: string
  displayName?: string | null
  hasTotp: boolean
  enrollmentPending?: boolean
  automatic: boolean
  status: ReloginStatus
  message: string
  recovery?: { state: string, message: string, retryAt: string | null, retriesUsed?: number, maxRetries?: number }
  planType: string | null
  planTypeDisplay?: string | null
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
  lastAttemptAt?: string | null
  verifiedAt: string | null
  expiresAt: string | null
  importedAt?: string | null
  updatedAt: string
}
export interface ReloginAttempt {
  operation?: MaintenanceAction | null
  id: string
  source: 'library' | 'account_menu' | 'automatic' | 'enrollment'
  startedAt: string
  finishedAt: string | null
  result: 'running' | 'succeeded' | 'failed' | 'cancelled' | 'awaiting_workspace'
  message: string
  accountId: string | null
  workspaceId: string | null
  pushResult: 'not_requested' | 'pending' | 'running' | 'succeeded' | 'failed' | 'uncertain'
  pushMessage: string
  pushedAt: string | null
}
export interface ReloginHistory { items: ReloginAttempt[], nextCursor: string | null }
export function getReloginHistory(id: string, before?: string, options: RequestOptions = {}) {
  return request<ReloginHistory>({ url: '/api/admin/relogin/history', method: 'POST', data: { id, before }, ...options })
}
export function getReloginTotp(id: string, options: RequestOptions = {}) {
  return request<{ secret: string }>({ url: '/api/admin/relogin/totp', method: 'POST', data: { id }, ...options })
}

export interface ReloginSettings { concurrency: number, paused: boolean, maxRetries: number, retryIntervalSeconds: number }
export type ReloginSettingsUpdate = Pick<ReloginSettings, 'concurrency' | 'paused'> & Partial<Pick<ReloginSettings, 'maxRetries' | 'retryIntervalSeconds'>>
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
  return request<{ imported: number, revisions?: Record<string, number> }>({ url: '/api/admin/relogin/import', method: 'POST', data: { text, replaceExisting } })
}
export function saveImportedPlans(data: { revisions: Record<string, number>, mode: 'draft' | 'now' | 'scheduled', when?: string | null, newAccount: ReloginNewAccountForm, template?: AccountTemplateSelection }) {
  return request<ReloginBatchResult[]>({ url: '/api/admin/relogin/import/plans', method: 'POST', data })
}
export function enrollRelogin(data: { text: string, replaceExisting: boolean, settings: import('./accounts').AccountImportSettings, outboundProxyId?: string, qualityTemplate?: Pick<import('./quality-ops').QualityRuleTemplate, 'id' | 'revision'> }) {
  return request<{ ids: string[] }>({ url: '/api/admin/relogin/enroll', method: 'POST', data })
}
export function exportRelogin(ids: string[], format: 'json' | 'two_fa', options: RequestOptions = {}) {
  return request<{ files: { name: string, content: string }[] }>({ url: '/api/admin/relogin/export', method: 'POST', data: { ids, format, confirm: 'export_sensitive_relogin' }, timeout: 120000, ...options })
}
export function queueRelogin(ids: string[], workspaceMode: ReloginWorkspaceMode = 'original') {
  return request<ReloginBatchResult[]>({ url: '/api/admin/relogin/queue', method: 'POST', data: { ids, workspaceMode } })
}
export interface ReloginNewAccountForm {
  settings: import('./accounts').AccountImportSettings
  outboundProxyId?: string
  qualityTemplate?: Pick<import('./quality-ops').QualityRuleTemplate, 'id' | 'revision'>
}
export function pushRelogin(rows: Pick<ReloginEntry, 'id' | 'revision'>[], template?: AccountTemplateSelection, customName?: string, selections?: Record<string, ReloginPushSelection>, newAccount?: ReloginNewAccountForm) {
  const ids = rows.map(row => row.id)
  const revisions = Object.fromEntries(rows.map(row => [row.id, row.revision]))
  return request<ReloginBatchResult[]>({ url: '/api/admin/relogin/push', method: 'POST', data: { ids, revisions, ...(template ? { template } : {}), ...(customName ? { customName } : {}), ...(selections ? { selections } : {}), ...(newAccount ? { newAccount } : {}) }, timeout: 120000 })
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

export type MaintenanceAction = 'edit_material' | 'check_session' | 'devices' | 'workspaces' | 'security' | 'revoke_device' | 'logout_all' | 'logout_and_relogin' | 'authorize' | 'force_login' | 'switch_workspace' | 'prepare_totp' | 'activate_totp' | 'disable_totp'
export interface MaintenanceDevice { id: string, sessionId: string | null, deviceIdHash: string | null, name: string, platform: string, current: boolean, trusted: boolean, lastActiveAt: number | null }
export interface MaintenanceWorkspace { id: string, name: string, planType: string | null, available: boolean }
export interface MaintenanceView {
  action: MaintenanceAction | null
  state: string
  message: string
  checkedAt: string | null
  sessionState: string
  sessionCheckedAt: string | null
  devices: MaintenanceDevice[]
  workspaces: MaintenanceWorkspace[]
  totpEnabled: boolean | null
  passwordEnabled: boolean | null
  pendingTotp: boolean
  uncertain: boolean
  loginPaused: boolean
}
export interface MaintenanceMaterial {
  revision: number
  needsResolution: boolean
  password: string
  mfaSecret: string
  pendingTotpSecret: string | null
  history: { password: string, mfa_secret: string, saved_at: string, pending_totp?: { secret: string } | null }[]
}
export function maintainRelogin(data: { id: string, revision: number, action: MaintenanceAction, target?: string, deviceIdHash?: string, confirm?: string }) {
  return request<MaintenanceView>({ url: '/api/admin/relogin/maintain', method: 'POST', data, timeout: 310000, silent: true })
}
export function cancelRelogin(id: string, revision: number) {
  return request<void>({ url: '/api/admin/relogin/cancel', method: 'POST', data: { id, revision } })
}
export function getReloginMaterial(id: string, options: RequestOptions = {}) {
  return request<MaintenanceMaterial>({ url: '/api/admin/relogin/material', method: 'POST', data: { id }, ...options })
}
export function updateReloginMaterial(data: { id: string, revision: number, password: string, mfaSecret: string, confirmMaintenanceResolution?: boolean }) {
  return request<void>({ url: '/api/admin/relogin/material/update', method: 'POST', data, silent: true })
}

export interface ReloginEnrollmentPlan {
  state: 'draft' | 'scheduled' | 'queued' | 'running' | 'failed' | 'verification' | 'uncertain' | 'done' | 'completed_early'
  when: string | null
  config: import('./account-templates').AccountTemplateConfig
  template: AccountTemplateSelection | null
  customName: string | null
  purchaseCost: import('./accounts').AccountPurchaseUpdate | null
  qualityTemplate: Pick<import('./quality-ops').QualityRuleTemplate, 'id' | 'revision'> | null
  exitLabel: string
  credentialExitLabel: string
}
export function saveEnrollmentPlan(data: {
  id: string
  revision: number
  mode: 'draft' | 'now' | 'scheduled' | 'cancel'
  when?: string | null
  newAccount?: ReloginNewAccountForm
  template?: AccountTemplateSelection
}) {
  return request<void>({ url: '/api/admin/relogin/enrollment-plan', method: 'POST', data })
}
