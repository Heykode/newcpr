import { parseExcelModels } from '@/views/accounts/utils/schedulingForm'

export type Excel403Action = 'none' | 'pause_account' | 'disable_excel'

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
