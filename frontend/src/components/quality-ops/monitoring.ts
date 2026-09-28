import type { QualityMonitoring, QualityTemplateTarget } from '@/api/modules/quality-ops'

export function monitoringPresentation(monitor: QualityMonitoring) {
  if (!monitor.enabled)
    return { label: '监测暂停', tone: 'text-cp-text-secondary' }
  if (monitor.running)
    return { label: '检测中', tone: 'text-cp-primary' }
  if (monitor.pending)
    return { label: '已排队', tone: 'text-cp-primary' }
  return { label: '监测中', tone: 'text-cp-success' }
}

export function qualityTemplateTargets(accountIds: string[], monitoring: Record<string, QualityMonitoring>): QualityTemplateTarget[] {
  return accountIds.map(accountId => ({
    accountId,
    ruleId: monitoring[accountId]?.ruleId ?? null,
    revision: monitoring[accountId]?.revision ?? null,
  }))
}
