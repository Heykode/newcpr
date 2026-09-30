import type { RequestOptions } from '../request'
import request from '../request'

export type CleanupCategory = 'requests' | 'files' | 'captures' | 'audit'
export interface CleanupSelection { selected: boolean, retentionDays: number }
export interface CleanupConfig {
  enabled: boolean
  frequency: 'hourly' | 'six_hourly' | 'daily'
  dailyHour: number
  dailyMinute: number
  timezone: string
  requests: CleanupSelection
  files: CleanupSelection
  captures: CleanupSelection
  audit: CleanupSelection
}
export interface CleanupJob {
  id: string
  instanceId: string
  automatic: boolean
  config: CleanupConfig
  cutoffAt: string
  status: 'running' | 'succeeded' | 'failed' | 'cancelled'
  categoryIndex: number
  removed: number
  errors: CleanupCategory[]
}
export interface CleanupState {
  revision: number
  config: CleanupConfig
  nextRunAt: string | null
  job: CleanupJob | null
}
export interface CleanupPreview { revision: number, config: CleanupConfig, cutoffAt: string }
export interface CleanupFootprint {
  measuredAt: string
  items: { category: CleanupCategory, bytes: number | null }[]
}
const base = '/api/admin/log-cleanup'
export function getCleanupState(options: RequestOptions = {}) {
  return request<CleanupState>({ url: base, method: 'GET', ...options })
}
export function getCleanupUsage(options: RequestOptions = {}) {
  return request<CleanupFootprint>({ url: `${base}/usage`, method: 'GET', ...options })
}
export function saveCleanupConfig(revision: number, config: CleanupConfig) {
  return request<void>({ url: base, method: 'POST', data: { revision, config } })
}
export function previewCleanup() {
  return request<CleanupPreview>({ url: `${base}/preview`, method: 'POST' })
}
export function startCleanup(data: CleanupPreview) {
  return request<CleanupJob>({ url: `${base}/start`, method: 'POST', data })
}
export function cancelCleanup(id: string) {
  return request<void>({ url: `${base}/cancel`, method: 'POST', data: { id } })
}
