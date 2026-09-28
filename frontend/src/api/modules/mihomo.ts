import type { RequestOptions } from '../request'
import request from '../request'

export interface CountryFilter { mode: 'off' | 'include' | 'exclude', codes: string[], allowUnknown: boolean, dynamicProviderManaged: boolean }
export interface NodeCheck {
  checkedAt: string | null
  success: boolean | null
  latencyMs: number | null
  message: string | null
  exitIp: string | null
  countryCode: string | null
  countryName?: string | null
  region?: string | null
  city?: string | null
  quality: null | {
    score: number
    grade: string
    summary: string
    checkedAt: string
    checks: { name: string, status: string, success: boolean, httpStatus: number | null, cfRay: string | null, latencyMs: number, reason: string }[]
  }
}
export interface MihomoNode {
  name: string
  displayName: string
  subscriptionIds: string[]
  dynamic: boolean
  state: string
  countryCode: string | null
  countryCheckedAt: string | null
  countryError: string | null
  countryBlocked: boolean
  check: NodeCheck | null
}
export interface WarmStatus { target: number, ready: number, readySubscription: number, readyDynamic: number, eligible: number, checking: number, cooling: number, failureReasons: Record<string, number> }
export interface MihomoStatus {
  version: string
  installed: boolean
  running: boolean
  supported: boolean
  busy: boolean
  phase: string
  error: string | null
  endpoint: string
  subscriptionDownloadMode: 'auto' | 'proxy' | 'direct'
  subscriptionItems: { id: string, label: string, enabled: boolean, nodes: number, cached: boolean, updatedAt: string | null }[]
  dynamicProxies: number
  nodeStates: MihomoNode[]
  countryFilter: CountryFilter
  countryCodes: string[]
  bpsWarmPool: WarmStatus
  bpsIpWarmPool: WarmStatus
  codexWarmPool: WarmStatus
  codexIpWarmPool: WarmStatus
}
export type MihomoAction = 'install' | 'start' | 'stop' | 'subscription_add' | 'subscription_update' | 'subscription_rename' | 'subscription_refresh' | 'subscription_remove' | 'subscription_enable' | 'subscription_disable' | 'dynamic_append' | 'dynamic_replace' | 'dynamic_remove' | 'dynamic_clear' | 'disable' | 'recover' | 'probe' | 'country_filter' | 'country_scan' | 'country_probe' | 'download_mode'
export interface MihomoCommand { action: MihomoAction, target?: string, name?: string, subscriptions?: string[], dynamicProxies?: string[], countryFilter?: CountryFilter, downloadMode?: string }
export const getMihomo = (options: RequestOptions = {}) => request<MihomoStatus>({ url: '/api/admin/proxies/mihomo', method: 'GET', ...options })
export const updateMihomo = (data: MihomoCommand) => request<MihomoStatus>({ url: '/api/admin/proxies/mihomo', method: 'POST', data })
export const checkMihomoNode = (node: string, quality = false) => request<NodeCheck>({ url: '/api/admin/proxies/mihomo/check', method: 'POST', data: { node, quality }, timeout: 125000 })
