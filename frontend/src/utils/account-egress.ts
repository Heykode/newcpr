import type { RequestProxySource } from './request-proxy-source'

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

export const accountIpv6Modes = [
  'fixed_ipv6_reuse',
  'random_ipv6_reuse',
  'fixed_ipv6_fresh',
  'random_ipv6_fresh',
]

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
