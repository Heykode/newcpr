import { parseExcelModels } from '@/views/accounts/utils/schedulingForm'

export type Excel403Action = 'none' | 'pause_account' | 'disable_excel'

export interface ExcelRecoveryConfig {
  enabled: boolean
  intervalMinutes: number
}

export interface ExcelRecoveryView extends ExcelRecoveryConfig {
  nextProbeAt: string
  lastProbeAt: string | null
  lastResult: string | null
  lastModel: string | null
  recoveredAt: string | null
}

export function excelRecoveryResultLabel(result: string | null): string {
  return ({
    probing: '探测中',
    recovered: '完整探测成功，已恢复调度',
    request_failed: '请求失败或超时，保持暂停',
    response_mismatch: '回复校验未通过，保持暂停',
    cancelled: '探测已取消',
    interrupted: '上次探测中断，等待下次计划',
    superseded: '配置已变化，旧结果未应用',
    model_unavailable: '原失败模型不在 Excel 模型列表，未换模型探测',
  } as Record<string, string>)[result ?? ''] ?? '尚未探测'
}

export function excelRecoverySettings(enabled: boolean, interval: string): ExcelRecoveryConfig {
  const value = interval.trim()
  const minutes = Number(value)
  if (!/^\d+$/.test(value) || !Number.isInteger(minutes) || minutes < 1 || minutes > 10080)
    throw new Error('Excel 恢复探测间隔必须是 1 至 10080 分钟的整数')
  return { enabled, intervalMinutes: minutes }
}

export const EXCEL_403_OPTIONS = [
  { value: 'none', label: '不自动处理' },
  { value: 'pause_account', label: '暂停账号调度' },
  { value: 'disable_excel', label: '关闭Excel模式' },
]

export function accountExcel403Action(account: { excel403Action?: Excel403Action | null, excelAutoDisableOn403?: boolean | null }): Excel403Action {
  return account.excel403Action ?? (account.excelAutoDisableOn403 ? 'pause_account' : 'none')
}

export function excel403ActionLabel(action: Excel403Action): string {
  return EXCEL_403_OPTIONS.find(option => option.value === action)?.label ?? '不自动处理'
}

export interface ExcelSettings {
  excelRecovery?: ExcelRecoveryConfig
  responsesUpstream: 'codex' | 'excel'
  excelModelsFollowGlobal: boolean
  excelCacheCreationAsInput?: boolean
  excel403Action?: Excel403Action
  excelModels?: string[]
}

export function excelSettings(enabled: boolean, followGlobal: boolean, input: string, cacheCreationAsInput?: boolean, action?: Excel403Action): ExcelSettings {
  const models = followGlobal ? undefined : parseExcelModels(input)
  if (models === null)
    throw new Error('Excel 模型最多 64 个，每个名称最多 128 个字母、数字、点、下划线或连字符')
  return {
    responsesUpstream: enabled ? 'excel' : 'codex',
    excelModelsFollowGlobal: followGlobal,
    ...(cacheCreationAsInput !== undefined ? { excelCacheCreationAsInput: cacheCreationAsInput } : {}),
    ...(action !== undefined ? { excel403Action: enabled ? action : 'none' } : {}),
    ...(models !== undefined ? { excelModels: models } : {}),
  }
}
