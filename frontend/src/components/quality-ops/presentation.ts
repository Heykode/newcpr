import type { QualityRuleConfig } from '@/api/modules/quality-ops'

export const qualityActions: Record<string, string> = {
  template_restored: '已恢复应用模板前的账号配置',
  template_restore_blocked: '原配置暂不可恢复，将在后续通过时重试',
  template_recovery_released: '账号配置已变更，已放弃模板自动恢复',
  template_applied: '已应用账号模板',
  template_applied_probe_paused: '已应用账号模板并暂停状态探针',
  template_unavailable: '模板已删除或版本变化，请重新选择',
  template_blocked_references: '模板分组或代理不可用，未修改账号',
  template_blocked_settings: '模板代理或 IPv6 等设置冲突，未修改账号',
  template_blocked_account: '账号当前不可用或已被独立暂停，未应用模板',
  scheduling_paused: '已暂停账号调度',
  groups_removed: '已移出指定分组',
  restored: '已恢复质量检测前的状态',
  restore_blocked: '恢复受阻：账号状态或分组已变化，请检查',
  ownership_released: '已保留人工设置，不自动恢复',
  already_applied: '保持此前的质量处置',
  no_change: '账号原已暂停或不在指定分组，未改动',
  identity_changed: '检测期间账号身份已变化，未执行处置',
  excel_enabled: '已开启 Excel 模式',
  excel_recovery_pending: '保持本规则的 Excel 处置，继续检测原生通道',
  excel_recovery_counted: '原生正常次数尚未达到关闭阈值，继续检测',
  excel_recovery_released: '账号或规则设置已变化，保留现状，不自动关闭 Excel',
  excel_disabled_native_recovered: '原生通道恢复正常，已关闭本规则开启的 Excel',
  excel_enabled_probe_paused: '已开启 Excel 模式，探针规则已暂停',
  probe_paused_excel: '账号已开启 Excel 模式，探针规则已暂停',
  excel_already_enabled: '账号已开启 Excel 模式',
  excel_blocked_configuration_changed: '检测期间配置已变化，未自动更改 Excel 或模板',
  excel_blocked_403: 'Excel 曾因 HTTP 403 关闭，未自动重开',
  excel_blocked_model: '检测模型未配置为 Excel 模型，未改动账号',
  excel_blocked_account: '账号当前不可用，未开启 Excel',
  excel_threshold_pending: '连续异常尚未达到阈值，未执行处置',
  failure_configuration_changed: '账号配置已变化，未执行处置',
  excel_streak_reset: '本轮正常，连续异常计数已清零',
}

const labels: Record<string, string> = {
  correct: '通过',
  incorrect: '检测异常',
  overloaded: '上游过载（待确认）',
  unknown: '不确定',
  request_error: '请求错误',
  running: '检测中',
  interrupted: '执行中断',
  cancelled: '已取消',
}

export function qualityVerdictLabel(status: string | null, mode?: QualityRuleConfig['detectionMode']) {
  if (mode === 'state_probe') {
    const probeLabels: Record<string, string> = { correct: '智商正常', incorrect: '智商异常', unknown: '无法判断', request_error: '无法判断' }
    if (status && probeLabels[status])
      return probeLabels[status]
  }
  return labels[status ?? ''] ?? status ?? '等待首次检测'
}

export function qualityStatusColor(status: string | null) {
  if (status === 'overloaded')
    return 'text-cp-warning'
  return status === 'correct' ? 'text-cp-success' : status === 'incorrect' || status === 'request_error' ? 'text-cp-error' : 'text-cp-text-secondary'
}
