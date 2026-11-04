import type { UsageListRecord } from '@/api'

// 只解释已记录的事实，不根据错误文案推测关闭原因。
export function usageErrorSummary(record: Partial<Pick<UsageListRecord, 'provider' | 'errorCode' | 'errorKind' | 'websocketCloseCode' | 'continuationUnavailableReason' | 'upstreamConnectionExitReason'>>): string | null {
  if (record.websocketCloseCode != null) {
    const reasons: Record<number, string> = { 1012: '上游服务重启', 1013: '上游繁忙', 1009: '消息体过大' }
    return reasons[record.websocketCloseCode] ?? '上游关闭连接'
  }
  if (record.errorKind === 'process_interrupted')
    return '服务进程中断'
  if (record.errorKind === 'continuation_recovery_required') {
    return record.continuationUnavailableReason === 'reused_connection_lost' && record.upstreamConnectionExitReason === 'max_age_expired'
      ? '续接连接已到期，需要恢复上下文'
      : '续接上下文不可用'
  }
  if (record.provider === 'openai') {
    const reasons: Record<string, string> = {
      previous_response_not_found: '续接上下文不可用',
      websocket_connection_limit_reached: '连接达到上游使用上限',
      context_length_exceeded: '上下文过长',
      insufficient_quota: '上游额度不足',
      rate_limit_exceeded: '上游限流',
      server_is_overloaded: '上游服务过载',
    }
    if (record.errorCode && reasons[record.errorCode])
      return reasons[record.errorCode]!
  }
  return record.upstreamConnectionExitReason === 'upstream_close' ? '上游关闭连接' : null
}
