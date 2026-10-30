import type { RequestOptions } from '../request'
import type { AccountGroupRef } from './account-groups'
import request from '../request'

export interface RequestLocation {
  country: string
  region: string
  city: string
  timezone: string
}

export function defaultRequestLocation(): RequestLocation {
  return { country: 'US', region: 'Ohio', city: 'Piketon', timezone: 'America/New_York' }
}

export interface OutboundProxyTest {
  location?: ProxyLocationDetection
  success: boolean
  latencyMs: number
  exitIp: string | null
  exitIpv4: string | null
  exitIpv6: string | null
  message: string
}

export type ProxyLocationDetection
  = | { status: 'notRequested' }
    | { status: 'detected', location: RequestLocation }
    | { status: 'failed', message: string }
    | { status: 'conflict' }

export interface DetectedProxyLocation {
  location: RequestLocation
  exitIpv4: string | null
  exitIpv6: string | null
  detectedAt: string
}

export interface OutboundProxyRecord {
  maxWebsocketConnections: number
  enabled: boolean
  autoLocation?: boolean
  detectedLocation?: DetectedProxyLocation | null
  effectiveLocation?: RequestLocation | null
  requestLocation?: RequestLocation | null
  id: string
  name: string
  endpoint: string
  hasAuthentication: boolean
  revision: number
  accountCount: number
  lastTestAt: string | null
  lastTest: OutboundProxyTest | null
  createdAt: string
  updatedAt: string
}

interface ProxyPage {
  items: OutboundProxyRecord[]
  page: { page: number, pageSize: number, total: number, totalPages: number }
}

export interface OutboundProxyAccount {
  id: string
  name: string
  email: string | null
  provider: string
  authenticationKind: string
  planType: string | null
  planTypeDisplay: string | null
  groups: AccountGroupRef[]
  enabled: boolean
}

interface ProxyAccountPage {
  items: OutboundProxyAccount[]
  page: ProxyPage['page']
}

interface ProxyListParams {
  page: number
  pageSize: number
  search?: string
}

interface ProxyAccountListParams extends ProxyListParams {
  proxyId: string
}

interface ProxyAccountRemoveParam {
  proxyId: string
  accountId: string
}

interface ProxyCreateParam {
  name: string
  proxyUrl: string
  maxWebsocketConnections?: number
  autoLocation?: boolean
  requestLocation?: RequestLocation | null
}

interface ProxyIdParam {
  id: string
  revision: number
}

interface ProxyTestParam extends ProxyIdParam {
  detectLocation?: boolean
}

interface ProxyUpdateParam extends ProxyIdParam, Omit<ProxyCreateParam, 'proxyUrl'> {
  enabled?: boolean
  proxyUrl?: string
}

interface ProxyProbeParam {
  proxyUrl: string
  detectLocation?: boolean
}

interface ProxyMutationResponse {
  configRevision: number
}

export function getProxyAccounts(data: ProxyAccountListParams, options: RequestOptions = {}) {
  return request<ProxyAccountPage>({
    url: '/api/admin/proxies/accounts',
    method: 'GET',
    params: data,
    ...options,
  })
}

export function removeProxyAccount(data: ProxyAccountRemoveParam, options: RequestOptions = {}) {
  return request<ProxyMutationResponse>({
    url: '/api/admin/proxies/accounts/remove',
    method: 'POST',
    data,
    ...options,
  })
}

interface ProxyMutation extends ProxyMutationResponse {
  record: OutboundProxyRecord
}

export type ProxyBatchResult<T> = { status: 'succeeded', data: T } | { status: 'failed', message: string, httpStatus: number, code: number } | { status: 'skipped' }

export function createProxyBatch(data: ProxyCreateParam[], options: RequestOptions = {}) {
  return request<ProxyBatchResult<ProxyMutation>[]>({ url: '/api/admin/proxies/create-batch', method: 'POST', data, timeout: 0, ...options })
}

export function testProxyBatch(data: ProxyTestParam[], options: RequestOptions = {}) {
  return request<ProxyBatchResult<OutboundProxyRecord>[]>({ url: '/api/admin/proxies/test-batch', method: 'POST', data, timeout: 0, ...options })
}

export function getProxies(data: ProxyListParams, options: RequestOptions = {}) {
  return request<ProxyPage>({
    url: '/api/admin/proxies',
    method: 'GET',
    params: data,
    ...options,
  })
}

export function createProxy(data: ProxyCreateParam, options: RequestOptions = {}) {
  return request<ProxyMutation>({
    url: '/api/admin/proxies/create',
    method: 'POST',
    data,
    timeout: 30000,
    ...options,
  })
}

export function updateProxy(data: ProxyUpdateParam, options: RequestOptions = {}) {
  return request<ProxyMutation>({
    url: '/api/admin/proxies/update',
    method: 'POST',
    data,
    timeout: 30000,
    ...options,
  })
}

export function deleteProxy(data: ProxyIdParam, options: RequestOptions = {}) {
  return request<ProxyMutationResponse>({
    url: '/api/admin/proxies/delete',
    method: 'POST',
    data,
    ...options,
  })
}

export function testProxy(data: ProxyTestParam, options: RequestOptions = {}) {
  return request<OutboundProxyRecord>({
    url: '/api/admin/proxies/test',
    method: 'POST',
    data,
    timeout: 25000,
    ...options,
  })
}

export function probeProxy(data: ProxyProbeParam, options: RequestOptions = {}) {
  return request<OutboundProxyTest>({
    url: '/api/admin/proxies/probe',
    method: 'POST',
    data,
    timeout: 25000,
    ...options,
  })
}

export function probeDirectLocation(options: RequestOptions = {}) {
  return request<OutboundProxyTest>({
    url: '/api/admin/proxies/probe-direct',
    method: 'POST',
    timeout: 25000,
    ...options,
  })
}
