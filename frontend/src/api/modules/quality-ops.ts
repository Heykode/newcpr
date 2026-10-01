import type { RequestOptions } from '../request'
import type { AccountTemplate } from './account-templates'
import request from '../request'

export interface QualityRuleConfig {
  detectionMode: 'answer' | 'state_probe'
  accountId: string
  model: string
  enabled: boolean
  intervalSeconds?: number | null
  cron: string
  timezone: string
  repetitions: number
  prompt: string
  referenceAnswer: string
  reasoningEffort: string | null
  judgeGroupId: string
  judgeModel: string
  judgePrompt: string
  failureAction: 'none' | 'disable_scheduling' | 'remove_groups' | 'enable_excel' | 'apply_account_template'
  failureTemplate?: AccountTemplate | null
  failureGroupIds: string[]
  autoRestore: boolean
  disableExcelOnNativeRecovery: boolean
  excelFailureThreshold: number
  excelRecoveryThreshold?: number
}

export interface QualityRule {
  id: string
  revision: number
  config: QualityRuleConfig
  nextRunAt: string
  running: boolean
  pending: boolean
  lastStatus: string | null
  lastRunAt: string | null
  lastAction?: string | null
  excelFailureStreak?: number
  sourceTemplate?: QualityRuleTemplateRef | null
}

export interface QualityRuleTemplateRef {
  id: string
  revision: number
  name: string
}

export interface QualityRuleTemplate extends QualityRuleTemplateRef {
  config: Omit<QualityRuleConfig, 'accountId'>
}

export interface QualityGroupFilter {
  group: string
  statuses: string[]
}
export interface QualityGroupRule {
  id: string
  revision: number
  name: string
  filter: QualityGroupFilter
  config: Omit<QualityRuleConfig, 'accountId'>
  ruleCount: number
  excludedCount: number
  lastSyncedAt: string | null
}
export interface QualityGroupUpdate {
  group: QualityGroupRule
  sync: { created: number, updated: number, failed: number }
}

export interface QualityMonitoring {
  ruleId: string
  revision: number
  enabled: boolean
  running: boolean
  pending: boolean
  nextRunAt: string
  lastStatus: string | null
  lastRunAt: string | null
  lastAction: string | null
  sourceTemplate?: QualityRuleTemplateRef | null
}

export interface QualityTemplateTarget {
  accountId: string
  ruleId: string | null
  revision: number | null
}

export interface QualityTemplateApplyResult {
  accountId: string
  ruleId: string | null
  success: boolean
  message: string | null
}

export interface QualityAnswer {
  index: number
  answer: string
  verdict: 'correct' | 'incorrect' | 'unknown' | 'request_error'
  reason: string
  elapsedMs: number
  returnedModel: string | null
  judgeAccountId: string | null
  probe?: {
    verdict: 'healthy' | 'degraded' | 'inconclusive'
    reason: string
    shots: { transport: string | null, status: number | null, ticketLength: number, changed: boolean | null, reason: string | null }[]
  } | null
}

export interface QualityRun {
  detectionMode?: QualityRuleConfig['detectionMode']
  id: string
  ruleId: string
  accountId: string
  model: string
  status: string
  startedAt: string
  finishedAt: string | null
  correct: number
  incorrect: number
  unknown: number
  requestErrors: number
  action?: string | null
  config?: QualityRuleConfig
  answers?: QualityAnswer[]
}

const base = '/api/admin/quality-ops'
export function getQualityGroups(options: RequestOptions = {}) {
  return request<QualityGroupRule[]>({ url: `${base}/groups`, method: 'GET', ...options })
}
export function saveQualityGroup(data: { id: string | null, revision: number | null, name: string, filter: QualityGroupFilter, config: QualityGroupRule['config'] }) {
  return request<QualityGroupUpdate>({ url: `${base}/groups/save`, method: 'POST', data })
}
export function deleteQualityGroup(data: { id: string, revision: number, deleteRules: boolean }) {
  return request<void>({ url: `${base}/groups/delete`, method: 'POST', data })
}
export function getQualityRules(options: RequestOptions = {}) {
  return request<QualityRule[]>({ url: `${base}/rules`, method: 'GET', ...options })
}
export function saveQualityRule(data: { id: string | null, revision: number | null, config: QualityRuleConfig }) {
  return request<QualityRule>({ url: `${base}/save`, method: 'POST', data })
}
export function deleteQualityRule(data: { id: string, revision: number }) {
  return request<void>({ url: `${base}/delete`, method: 'POST', data })
}
export function runQualityRule(data: { id: string, revision: number }) {
  return request<void>({ url: `${base}/run`, method: 'POST', data })
}
export function getQualityRuns(id: string, options: RequestOptions = {}) {
  return request<QualityRun[]>({ url: `${base}/runs`, method: 'GET', params: { id }, ...options })
}
export function getQualityDetail(id: string, options: RequestOptions = {}) {
  return request<QualityRun>({ url: `${base}/detail`, method: 'GET', params: { id }, ...options })
}

export function getQualityTemplates(options: RequestOptions = {}) {
  return request<QualityRuleTemplate[]>({ url: `${base}/templates`, method: 'GET', ...options })
}
export function saveQualityTemplate(data: { id: string | null, revision: number | null, name: string, config: QualityRuleTemplate['config'] }) {
  return request<QualityRuleTemplate>({ url: `${base}/templates/save`, method: 'POST', data })
}
export function deleteQualityTemplate(data: { id: string, revision: number }) {
  return request<void>({ url: `${base}/templates/delete`, method: 'POST', data })
}
export function applyQualityTemplate(data: { id: string, revision: number, targets: QualityTemplateTarget[] }) {
  return request<QualityTemplateApplyResult[]>({ url: `${base}/templates/apply`, method: 'POST', data, silent: true })
}
export function getQualityMonitoring(accountIds: string[], options: RequestOptions = {}) {
  return request<Record<string, QualityMonitoring>>({ url: `${base}/monitoring`, method: 'POST', data: { accountIds }, ...options })
}
