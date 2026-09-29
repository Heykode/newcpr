import type { RequestProxySource } from './request-proxy-source'
import type { Ipv6EgressConfig } from '@/api/modules/ipv6-egress'

export interface AccountEgressDraft {
  proxyMode: string
  proxyId: string
  egressMode: string
}

export interface AccountEgressPatch {
  requestProxySource?: RequestProxySource
  outboundProxyId?: string
  egressMode?: string | null
}

export interface AccountEgressReadState {
  config?: Ipv6EgressConfig
  loading: boolean
  error: boolean
}

export const accountIpv6Modes = [
  'fixed_ipv6_reuse',
  'random_ipv6_reuse',
  'fixed_ipv6_fresh',
  'random_ipv6_fresh',
]

export function accountEgressFromAccount(account: {
  id: string
  provider: string
  requestProxySource?: RequestProxySource
  outboundProxyEndpoint?: string | null
}, config?: Ipv6EgressConfig): AccountEgressDraft | undefined {
  const draft = { proxyMode: '', proxyId: '', egressMode: 'fixed_ipv6_reuse' }
  if (account.provider === 'openai' && (account.requestProxySource === 'mihomo' || account.requestProxySource === 'proxy_pool'))
    return { ...draft, proxyMode: account.requestProxySource }
  // The redacted endpoint is display-only, not a saved proxy ID or connection URL.
  if (account.outboundProxyEndpoint)
    return { ...draft, proxyMode: 'proxy' }
  if (account.provider !== 'openai')
    return { ...draft, proxyMode: 'direct' }
  if (!config)
    return undefined
  const mode = config.accountOverrides[account.id]
  if (mode == null)
    return { ...draft, proxyMode: 'inherit' }
  if (mode === 'unchanged')
    return { ...draft, proxyMode: 'direct' }
  if (accountIpv6Modes.includes(mode))
    return { ...draft, proxyMode: 'ipv6', egressMode: mode }
  return undefined
}

export function sameAccountEgress(left: AccountEgressDraft, right: AccountEgressDraft): boolean {
  return left.proxyMode === right.proxyMode
    && (left.proxyMode !== 'proxy' || left.proxyId.trim() === right.proxyId.trim())
    && (left.proxyMode !== 'ipv6' || left.egressMode === right.egressMode)
}

// Omitted fields preserve existing settings. Pool leases override dormant account
// settings at request time, so selecting a pool must not erase those settings.
export function accountEgressPatch(draft: AccountEgressDraft, openai: boolean): AccountEgressPatch {
  const { proxyMode, proxyId, egressMode } = draft
  if (proxyMode === 'preserve')
    return {}
  if (!openai && !['direct', 'proxy'].includes(proxyMode))
    throw new Error('当前账号平台仅支持服务器直连或指定代理')
  if (proxyMode === 'mihomo' || proxyMode === 'proxy_pool')
    return { requestProxySource: proxyMode }
  if (proxyMode === 'proxy' && !proxyId.trim())
    throw new Error('请选择已通过测试的代理')
  if (proxyMode === 'ipv6' && !accountIpv6Modes.includes(egressMode))
    throw new Error('请选择 IPv6 出口策略')
  if (!['inherit', 'direct', 'proxy', 'ipv6'].includes(proxyMode))
    throw new Error('请选择出站隧道')
  return {
    requestProxySource: 'account',
    outboundProxyId: proxyMode === 'proxy' ? proxyId.trim() : '',
    ...(openai
      ? {
          egressMode: proxyMode === 'inherit' ? null : proxyMode === 'ipv6' ? egressMode : 'unchanged',
        }
      : {}),
  }
}
