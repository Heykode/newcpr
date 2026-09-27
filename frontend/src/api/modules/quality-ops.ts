import type { RequestOptions } from '../request'
import request from '../request'

export interface QualityRuleConfig {
  accountId: string
  model: string
  enabled: boolean
  cron: string
  timezone: string
  repetitions: number
  prompt: string
  referenceAnswer: string
  reasoningEffort: string | null
  judgeGroupId: string
  judgeModel: string
  judgePrompt: string
  failureAction: 'none' | 'disable_scheduling' | 'remove_groups'
  failureGroupIds: string[]
  autoRestore: boolean
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
}

export interface QualityAnswer {
  index: number
  answer: string
  verdict: 'correct' | 'incorrect' | 'unknown' | 'request_error'
  reason: string
  elapsedMs: number
  returnedModel: string | null
  judgeAccountId: string | null
}

export interface QualityRun {
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
