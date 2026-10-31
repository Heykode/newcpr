import type { RequestOptions } from '../request'
import request from '../request'

export type OutboundUserAgentSelection
  = | { mode: 'default' }
    | { mode: 'configuration', configuration: ClientProfileConfiguration }
    | { mode: 'custom', userAgent: string }
    | { mode: 'custom-auto-version', userAgent: string }

export interface OutboundUserAgentSettings {
  mode: 'default' | 'custom' | 'custom-auto-version' | 'configuration'
  configuration?: ClientProfileConfiguration | null
  profileOptions?: { presets: { configuration: PresetProfile, automaticAvailable: boolean, reason: string | null, defaults: { originator: string, osType: string, osVersion: string, arch: string, terminal: string } }[], maxVersionLag: number } | null
  profilePreview?: { userAgent: string, versionSource: string, checkedAt: string | null, verifiedAt: string | null, error: string | null, codexVersion: string, desktopVersion: string | null, desktopBuild: string | null } | null
  customUserAgent: string | null
  defaultUserAgent: string
  effectiveUserAgent: string
  effectiveDesktopUserAgent: string
  coreVersion: string
  desktopVersion: string
  osType: string
  osVersion: string
  arch: string
  terminal: string
  verified: boolean
  defaultVerifiedAt: string
}

export interface PresetProfile {
  mode?: never
  client: 'desktop' | 'cli'
  platform: 'macos' | 'linux' | 'windows'
  versionMode: 'latest' | 'fixed'
  versionLag?: number | null
  cliEntry?: 'tui' | 'exec' | null
  originator?: string | null
  osType?: string | null
  osVersion?: string | null
  arch?: string | null
  terminal?: string | null
  codexVersion?: string | null
  desktopVersion?: string | null
  desktopBuild?: string | null
}

export type ClientProfileConfiguration = PresetProfile | {
  mode: 'custom'
  userAgent: string
  originator?: string | null
  codexVersion?: string | null
}

const endpoint = '/api/admin/settings/openai-user-agent'

export function getOutboundUserAgent(options: RequestOptions = {}) {
  return request<OutboundUserAgentSettings>({ url: endpoint, method: 'GET', ...options })
}

export function previewOutboundUserAgent(data: OutboundUserAgentSelection, options: RequestOptions = {}) {
  return request<OutboundUserAgentSettings>({
    url: `${endpoint}/preview`,
    method: 'POST',
    data,
    ...options,
  })
}

export function updateOutboundUserAgent(data: OutboundUserAgentSelection, options: RequestOptions = {}) {
  return request<OutboundUserAgentSettings>({ url: endpoint, method: 'POST', data, ...options })
}
