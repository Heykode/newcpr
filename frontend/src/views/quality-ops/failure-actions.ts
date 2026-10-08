import type { QualityRuleConfig } from '@/api/modules/quality-ops'

export function usesFailureThreshold(action: QualityRuleConfig['failureAction']): boolean {
  return action !== 'none'
}

export function failureThreshold(config: Partial<QualityRuleConfig>): number {
  return config.failureAction === 'disable_scheduling' || config.failureAction === 'remove_groups'
    ? config.failureThreshold ?? 1
    : config.excelFailureThreshold ?? 1
}

export function setFailureThreshold(config: QualityRuleConfig, value: number) {
  if (config.failureAction === 'disable_scheduling' || config.failureAction === 'remove_groups')
    config.failureThreshold = value
  else
    config.excelFailureThreshold = value
}

export function failureActionOptions(current: QualityRuleConfig['failureAction']) {
  return [
    { value: 'none', label: '仅记录结果' },
    { value: 'disable_scheduling', label: '暂停此账号调度' },
    { value: 'remove_groups', label: '移出指定分组' },
    { value: 'apply_account_template', label: '应用账号模板' },
    ...(current === 'enable_excel' ? [{ value: 'enable_excel', label: '开启Excel模式（旧规则）' }] : []),
  ]
}
