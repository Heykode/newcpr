import type {
  CodexQuotaOverdraftState,
  CodexQuotaOverdraftWindow,
} from '@/api/modules/accounts'
import { formatDateTime } from '@/utils/date'

export type CodexQuotaOverdraftTone = 'danger' | 'success' | 'warning'

export interface CodexQuotaOverdraftStatusView {
  label: string
  detail: string
  title: string
  tone: CodexQuotaOverdraftTone
}

const reasonLabels: Record<string, string> = {
  authentication_failed: '认证失败',
  feature_disabled: '功能已关闭',
  fingerprint_failed: '身份画像生成失败',
  invalid_response: '上游响应无效',
  model_unavailable: '探测模型不可用',
  payload_failed: '探测请求生成失败',
  quota_limited: '额度仍不可用',
  request_failed: '探测请求失败',
  transient_failure: '上游暂时不可用',
  upstream_unavailable: '上游暂时不可用',
}

export function codexQuotaOverdraftStatusView(
  state: CodexQuotaOverdraftState,
  now: number,
): CodexQuotaOverdraftStatusView {
  const attempts = Math.max(0, state.attempts || 0)
  const limit = Math.max(1, state.attemptLimit || 5)
  const progress = `${attempts}/${limit}`
  const window = quotaWindowLabel(state.quotaWindow)
  const reason = state.reasonCode ? reasonLabels[state.reasonCode] ?? state.reasonCode : ''
  const title = [
    window,
    state.probeModel,
    reason,
    state.testedAt ? `最近探测：${formatDateTime(state.testedAt)}` : '',
    state.retryAt ? `下次重试：${formatDateTime(state.retryAt)}` : '',
    state.recoverAt ? `预计恢复：${formatDateTime(state.recoverAt)}` : '',
  ].filter(Boolean).join(' · ')

  switch (state.status) {
    case 'pending':
      return { label: attempts === 0 ? '等待探测' : '额度探测中', detail: [progress, state.probeModel].filter(Boolean).join(' · '), title, tone: 'warning' }
    case 'passed':
      return { label: '透支调度中', detail: window, title, tone: 'success' }
    case 'inconclusive':
      return { label: '探测无结论', detail: [progress, futureTime(state.retryAt, now, '后重试')].filter(Boolean).join(' · '), title, tone: 'warning' }
    case 'failed':
      return { label: reason === '认证失败' ? '认证失败' : '透支不可用', detail: futureTime(state.recoverAt, now, '后恢复'), title, tone: 'danger' }
    case 'recovered':
      return { label: '额度已恢复', detail: window, title, tone: 'success' }
  }
}

export function codexQuotaOverdraftWindow(
  state: CodexQuotaOverdraftState | null | undefined,
  windowKey: string,
): CodexQuotaOverdraftWindow | null {
  return state?.windows.find(window => window.windowKey === windowKey) ?? null
}

export function codexQuotaOverdraftStatsText(window: CodexQuotaOverdraftWindow): string {
  const stats = window.stats
  if (!stats)
    return ''
  return [stats.requestsDisplay || String(stats.requests), stats.tokensDisplay, stats.costUsdDisplay]
    .filter(Boolean)
    .join(' · ')
}

function quotaWindowLabel(value: string | null): string {
  if (!value)
    return ''
  if (value === 'multiple')
    return '5h / 7d'
  return value
}

function futureTime(value: string | null, now: number, suffix: string): string {
  if (!value)
    return ''
  const target = new Date(value).getTime()
  if (!Number.isFinite(target) || target <= now)
    return ''
  const seconds = Math.max(1, Math.ceil((target - now) / 1000))
  if (seconds < 60)
    return `${seconds} 秒${suffix}`
  const minutes = Math.ceil(seconds / 60)
  if (minutes < 60)
    return `${minutes} 分钟${suffix}`
  const hours = Math.ceil(minutes / 60)
  return `${hours} 小时${suffix}`
}
