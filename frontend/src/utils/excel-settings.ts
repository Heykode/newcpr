import { parseExcelModels } from '@/views/accounts/utils/schedulingForm'

export interface ExcelSettings {
  responsesUpstream: 'codex' | 'excel'
  excelModelsFollowGlobal: boolean
  excelModels?: string[]
}

export function excelSettings(enabled: boolean, followGlobal: boolean, input: string): ExcelSettings {
  const models = followGlobal ? undefined : parseExcelModels(input)
  if (models === null)
    throw new Error('Excel 模型最多 64 个，每个名称最多 128 个字母、数字、点、下划线或连字符')
  return {
    responsesUpstream: enabled ? 'excel' : 'codex',
    excelModelsFollowGlobal: followGlobal,
    ...(models !== undefined ? { excelModels: models } : {}),
  }
}
