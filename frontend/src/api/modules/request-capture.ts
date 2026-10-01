import type { RequestOptions } from '../request'
import { API_BASE_URL } from '../constants'
import request from '../request'

export interface CaptureConfig {
  enabled: boolean
  globalErrors: boolean
  includeMedia: boolean
  quotaMib: number
  quotaPolicy?: 'stop' | 'overwrite'
  retentionDays: number
}
export interface CaptureTask {
  id: string
  scope: 'key' | 'account' | 'group' | 'global'
  targetId: string
  includeMedia: boolean
  startedAt: string
  expiresAt: string
  status: 'running' | 'stopped' | 'expired' | 'interrupted'
}
export interface CaptureRecord {
  id: string
  taskId: string
  requestId: string
  bytes: number
  incomplete: boolean
  createdAt: string
}
export interface CaptureStatus {
  config: CaptureConfig
  instanceId: string
  tasks: CaptureTask[]
  records: CaptureRecord[]
  skipped: number
  activeSessions: number
  bufferedBytes: number
  storageFault: boolean
}
export interface CapturePage { text: string, nextOffset: number | null }
export interface CaptureSettings {
  config: CaptureConfig
  globalActive: boolean
  storageFault: boolean
  skipped: number
  storedBytes?: number
  recordCount?: number
}
export interface CaptureClearResult {
  cutoffAt: string
  removedRecords: number
  removedBytes: number
  complete: boolean
}
export function clearCapturedErrors(data: { confirmed: true, cutoffAt?: string }) {
  return request<CaptureClearResult>({ url: '/api/admin/request-captures/clear', method: 'POST', data })
}
export function getCaptureSettings(options: RequestOptions = {}) {
  return request<CaptureSettings>({ url: '/api/admin/request-captures/config', method: 'GET', ...options })
}
export function getCapturesForRequest(requestId: string, options: RequestOptions = {}) {
  return request<CaptureRecord[]>({ url: '/api/admin/request-captures/by-request', method: 'GET', params: { requestId }, ...options })
}
export function getRequestCaptures(options: RequestOptions = {}) {
  return request<CaptureStatus>({ url: '/api/admin/request-captures', method: 'GET', ...options })
}
export function configureRequestCaptures(data: CaptureConfig) {
  return request<void>({ url: '/api/admin/request-captures/config', method: 'POST', data })
}
export function createRequestCapture(data: Pick<CaptureTask, 'scope' | 'targetId' | 'includeMedia'> & { minutes: number }) {
  return request<CaptureTask>({ url: '/api/admin/request-captures', method: 'POST', data })
}
export function stopRequestCapture(id: string) {
  return request<void>({ url: '/api/admin/request-captures/stop', method: 'POST', data: { id } })
}
export function deleteRequestCapture(id: string) {
  return request<void>({ url: '/api/admin/request-captures/delete', method: 'POST', data: { id } })
}
export function readRequestCapture(id: string, offset: number, options: RequestOptions = {}) {
  return request<CapturePage>({ url: '/api/admin/request-captures/records', method: 'GET', params: { id, offset }, ...options })
}
export function captureExportUrl(kind: 'task' | 'record', id: string) {
  const segment = kind === 'record' ? 'records/' : ''
  return `${API_BASE_URL}/api/admin/request-captures/${segment}export?id=${encodeURIComponent(id)}`
}
