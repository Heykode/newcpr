import type { RequestOptions } from '../request'
import request from '../request'

export type OutboundTlsProfile = 'cpr' | 'qx-compatible'
export type OutboundSessionPolicy = 'native' | 'qx-compatible'

export type OutboundUserAgentSelection
  = | { mode: 'default' }
    | { mode: 'custom', userAgent: string }
    | { mode: 'qx-compatible', userAgent?: string | null }
    | { mode: 'independent', userAgent: string | null, tlsProfile: OutboundTlsProfile, sessionPolicy: OutboundSessionPolicy }

export interface OutboundUserAgentSettings {
  mode: 'default' | 'custom' | 'qx-compatible' | 'independent'
  customUserAgent: string | null
  tlsProfile: OutboundTlsProfile
  sessionPolicy: OutboundSessionPolicy
  defaultUserAgent: string
  qxDefaultUserAgent: string
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
