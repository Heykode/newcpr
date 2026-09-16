import type { RequestOptions } from '../request'
import request from '../request'

export interface Ipv6EgressAddress {
  id: string
  address: string
  enabled: boolean
}

export interface Ipv6EgressConfig {
  revision: number
  defaultMode: string
  addresses: Ipv6EgressAddress[]
  accountOverrides: Record<string, string | null>
  fixedBindings: Record<string, string>
}

export const ipv6EgressModes = [
  { value: 'unchanged', label: '不启用 IPv6 策略', description: '保留原直连或代理设置，不指定源地址' },
  { value: 'fixed_ipv6_reuse', label: '固定 IPv6 · 复用连接', description: '保留账号历史地址' },
  { value: 'random_ipv6_reuse', label: '随机 IPv6 · 复用连接', description: '独立连接选址，不逐帧轮换' },
  { value: 'fixed_ipv6_fresh', label: '固定 IPv6 · 新建连接', description: '保留账号历史地址' },
  { value: 'random_ipv6_fresh', label: '随机 IPv6 · 新建连接', description: '独立连接选址，续接优先原连接' },
]

interface Ipv6EgressMutation {
  configRevision: number
  config: Ipv6EgressConfig
}

export function getIpv6Egress(options: RequestOptions = {}) {
  return request<Ipv6EgressConfig>({ url: '/api/admin/ipv6-egress', method: 'GET', ...options })
}

export function updateIpv6Egress(data: {
  revision: number
  defaultMode: string
  addresses: Ipv6EgressAddress[]
}, options: RequestOptions = {}) {
  return request<Ipv6EgressMutation>({ url: '/api/admin/ipv6-egress/update', method: 'POST', data, ...options })
}

export function updateAccountIpv6Egress(data: {
  accountId: string
  revision: number
  mode: string | null
}, options: RequestOptions = {}) {
  return request<Ipv6EgressMutation>({ url: '/api/admin/ipv6-egress/account', method: 'POST', data, ...options })
}

export function expandIpv6Egress(data: { start: string, end: string }, options: RequestOptions = {}) {
  return request<Ipv6EgressAddress[]>({ url: '/api/admin/ipv6-egress/expand', method: 'POST', data, ...options })
}
