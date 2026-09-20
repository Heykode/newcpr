import type { RequestOptions } from '../request'
import type { RequestLocation } from './proxies'
import request from '../request'

export type RotationStrategy = 'smart' | 'quota_reset_priority' | 'round_robin' | 'sticky'

export interface RequestTuning {
  maxAccountSwitches: number
  maxRequestAttempts: number
  websocketMaxRetries: number
  websocketHttpFallbackEnabled: boolean
  websocketLargeRequestThresholdBytes: number
  websocketMaxAgeMs: number
  websocketStreamIdleTimeoutMs: number
  websocketFailureThreshold: number
  websocketFailureWindowMs: number
  websocketFailureOpenDurationMs: number
  rateLimitCooldownSeconds: number
  openaiLocationOverrideEnabled: boolean
  openaiRequestLocation: RequestLocation | null
  maxWaitingPerKey: number
  keyConcurrencyWaitTimeoutSeconds: number
  accountBusyWaitEnabled: boolean
  accountBusyWaitStickyMaxWaiting: number
  accountBusyWaitStickyTimeoutSeconds: number
  accountBusyWaitFallbackMaxWaiting: number
  accountBusyWaitFallbackTimeoutSeconds: number
}

export type RequestTuningOverrides = {
  [Key in keyof RequestTuning]?: RequestTuning[Key] | null
}

export interface RuntimeSettings {
  disableFast?: boolean
  turnStateInjectionEnabled?: boolean
  turnStateModels?: string[]
  turnStateProbeProxyId?: string | null
  turnStateProbeConcurrency?: number
  responsesMaxDecompressedBodyBytes?: number
  modelMappings: Record<string, string>
  refreshMarginSeconds: number
  refreshConcurrency: number
  maxConcurrentPerAccount: number
  requestIntervalMs: number
  rotationStrategy: RotationStrategy
  minCodexDesktopVersion: string | null
  minCodexCliVersion: string | null
  usageRetentionDays: number
  opsEventRetentionDays: number
  auditRetentionDays: number
  requestTuning?: RequestTuningOverrides
  requestTuningDefaults?: Partial<RequestTuning>
  updatedAt: string
}

export type ClientArchitecture = 'x64' | 'arm64'
export type ClientDownloadSource = 'microsoft_store' | 'official_openai'

export interface ClientDownloadPackage {
  architecture: ClientArchitecture
  source: ClientDownloadSource
  version: string | null
  fileName: string
  sizeBytes: number | null
  downloadUrl: string
  expiresAt: string | null
}

export interface CodexDesktopWindowsDownloads {
  resolvedAt: string
  cached: boolean
  warning: string | null
  packages: ClientDownloadPackage[]
}

export interface AdminApiKeyStatus {
  exists: boolean
}

export interface RegeneratedAdminApiKey {
  key: string
}

export interface DeletedAdminApiKey {
  message: string
}

export function getSettings(options: RequestOptions = {}) {
  return request<RuntimeSettings>({
    url: '/api/admin/settings',
    method: 'GET',
    ...options,
  })
}

type UpdateSettingsParam = Omit<RuntimeSettings, 'updatedAt' | 'requestTuningDefaults'>

export function updateSettings(data: UpdateSettingsParam, options: RequestOptions = {}) {
  return request<RuntimeSettings>({
    url: '/api/admin/settings/update',
    method: 'POST',
    data,
    ...options,
  })
}

export function getAdminApiKeyStatus(options: RequestOptions = {}) {
  return request<AdminApiKeyStatus>({
    url: '/api/admin/settings/admin-api-key',
    method: 'GET',
    ...options,
  })
}

export function regenerateAdminApiKey(options: RequestOptions = {}) {
  return request<RegeneratedAdminApiKey>({
    url: '/api/admin/settings/admin-api-key/regenerate',
    method: 'POST',
    ...options,
  })
}

export function deleteAdminApiKey(options: RequestOptions = {}) {
  return request<DeletedAdminApiKey>({
    url: '/api/admin/settings/admin-api-key/delete',
    method: 'POST',
    ...options,
  })
}

export function getCodexDesktopWindowsDownloads(refresh = false, options: RequestOptions = {}) {
  return request<CodexDesktopWindowsDownloads>({
    url: '/api/admin/settings/client-downloads/codex-desktop/windows',
    method: 'GET',
    params: refresh ? { refresh: true } : undefined,
    ...options,
  })
}
