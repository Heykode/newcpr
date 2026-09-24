import type { OutboundProxyRecord, OutboundProxyTest, ProxyLocationDetection } from '@/api/modules/proxies'

export function locationDetectionMessage(result?: ProxyLocationDetection | null): string {
  if (result?.status === 'failed')
    return result.message
  if (result?.status === 'conflict')
    return 'IPv4 与 IPv6 出口时区不同，自动地区不可用'
  if (result?.status === 'detected')
    return `${result.location.city} · ${result.location.timezone}`
  return '尚未识别出口地区'
}

export function proxyLocationMessage(proxy: OutboundProxyRecord): string {
  if (!proxy.autoLocation)
    return proxy.requestLocation ? `${proxy.requestLocation.city} · ${proxy.requestLocation.timezone}` : '继承全局'
  const detected = proxy.detectedLocation
  const result = proxy.lastTest?.location
  if (result?.status === 'failed' || result?.status === 'conflict') {
    const message = locationDetectionMessage(result)
    return detected ? `${message}；保留 ${detected.location.timezone}` : message
  }
  return detected
    ? `${detected.location.city} · ${detected.location.timezone}`
    : locationDetectionMessage(result)
}

export function proxyTestFeedback(result: OutboundProxyTest): { tone: 'success' | 'warning' | 'error', message: string } {
  if (!result.success)
    return { tone: 'error', message: result.message }
  if (result.location?.status === 'failed' || result.location?.status === 'conflict')
    return { tone: 'warning', message: `连接成功；${locationDetectionMessage(result.location)}` }
  return {
    tone: 'success',
    message: result.location?.status === 'detected'
      ? `连接成功；${locationDetectionMessage(result.location)}`
      : `连接成功，耗时 ${result.latencyMs} ms`,
  }
}
