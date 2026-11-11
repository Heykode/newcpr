import type { UsageListRecord } from '@/api'
import { t } from '@/i18n'

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

export function usageDisplayOutcome(outcome: string, errorCode: string | null | undefined, errorKind: string | null | undefined): string {
  // 已开始交付的请求仍可能明确失败，展示不改变原始终态
  // 进程中断和缺少明确错误的断流保留未完成，不从 HTTP 状态或文案推断
  if (outcome === 'incomplete' && errorKind !== 'process_interrupted' && (errorCode || errorKind === 'timeout'))
    return 'failed'
  return outcome
}

export function usageErrorTitle(record: { logicalOutcome: string, errorCode?: string | null, errorKind?: string | null }): string {
  return usageDisplayOutcome(record.logicalOutcome, record.errorCode, record.errorKind) === 'incomplete'
    ? t('usage.error.responseIsNotComplete')
    : t('usage.error.requestFailed')
}

export function usageErrorOrigin(record: { errorOrigin?: UsageListRecord['errorOrigin'] }) {
  const origins = {
    proxy: {
      label: t('usage.error.origin.proxy'),
      className: 'bg-cp-blue-container text-cp-blue-on-container',
      hint: null,
    },
    upstream: {
      label: t('usage.error.origin.upstream'),
      className: 'bg-cp-error-container text-cp-error-on-container',
      hint: null,
    },
    connection: {
      label: t('usage.error.origin.connection'),
      className: 'bg-cp-warning-container text-cp-warning-on-container',
      hint: t('usage.error.origin.connectionHint'),
    },
    unknown: {
      label: t('usage.error.origin.unknown'),
      className: 'bg-cp-fill-tertiary text-cp-text-secondary',
      hint: t('usage.error.origin.unknownHint'),
    },
  }
  return { kind: record.errorOrigin, ...origins[record.errorOrigin ?? 'unknown'] }
}

export function usageSendStateText(state: string) {
  switch (state) {
    case 'not_sent': return t('usage.error.sendState.notSent')
    case 'sent': return t('usage.error.sendState.sent')
    case 'ambiguous': return t('usage.error.sendState.ambiguous')
    case 'unknown': return t('usage.error.responseNotRecorded')
    default: return state
  }
}
