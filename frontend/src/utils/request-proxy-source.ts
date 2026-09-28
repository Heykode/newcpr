// Applied to account requests on either the native Codex or Excel route.
export type RequestProxySource = 'account' | 'mihomo' | 'proxy_pool'

export const requestProxySources = [
  { value: 'account', label: '跟随账号原出口' },
  { value: 'mihomo', label: 'Mihomo 会话代理池' },
  { value: 'proxy_pool', label: '普通代理池' },
] satisfies { value: RequestProxySource, label: string }[]
