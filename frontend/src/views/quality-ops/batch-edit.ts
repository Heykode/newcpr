import type { QualityRule, QualityRuleConfig } from '@/api/modules/quality-ops'

export const qualityEditableFields = [
  'enabled',
  'cron',
  'timezone',
  'detectionMode',
  'model',
  'reasoningEffort',
  'repetitions',
  'prompt',
  'referenceAnswer',
  'judgeGroupId',
  'judgeModel',
  'judgePrompt',
  'failureAction',
  'failureGroupIds',
  'autoRestore',
  'excelFailureThreshold',
] as const satisfies readonly (keyof QualityRuleConfig)[]

export type QualityEditableField = typeof qualityEditableFields[number]
export type QualityPatch = Partial<Pick<QualityRuleConfig, QualityEditableField>>

const answerFields = new Set<QualityEditableField>([
  'reasoningEffort',
  'repetitions',
  'prompt',
  'referenceAnswer',
  'judgeGroupId',
  'judgeModel',
  'judgePrompt',
])

export function buildQualityPatch(fields: readonly QualityEditableField[], draft: QualityRuleConfig): QualityPatch {
  const patch: QualityPatch = {}
  for (const field of qualityEditableFields) {
    if (fields.includes(field))
      Object.assign(patch, { [field]: Array.isArray(draft[field]) ? [...draft[field]] : draft[field] })
  }
  return patch
}

export function applyQualityPatch(source: QualityRuleConfig, patch: QualityPatch): QualityRuleConfig {
  const next = { ...source, failureGroupIds: [...source.failureGroupIds] }
  const mode = patch.detectionMode ?? source.detectionMode
  const action = patch.failureAction ?? source.failureAction
  for (const field of qualityEditableFields) {
    if (!Object.hasOwn(patch, field))
      continue
    if (mode === 'state_probe' && answerFields.has(field))
      continue
    if (field === 'failureGroupIds' && action !== 'remove_groups')
      continue
    if (field === 'excelFailureThreshold' && action !== 'enable_excel')
      continue
    Object.assign(next, { [field]: Array.isArray(patch[field]) ? [...patch[field]] : patch[field] })
  }
  if (Object.hasOwn(patch, 'detectionMode') && mode === 'state_probe') {
    next.repetitions = 1
    next.reasoningEffort = null
  }
  if (action === 'enable_excel')
    next.autoRestore = false
  return next
}

export interface QualityBatchResult {
  id: string
  success: boolean
  message: string
}

export async function saveQualityBatch(
  ids: readonly string[],
  patch: QualityPatch,
  ports: {
    read: () => Promise<QualityRule[]>
    save: (value: { id: string, revision: number, config: QualityRuleConfig }) => Promise<QualityRule>
    stopped: () => boolean
    onResult: (result: QualityBatchResult) => void
  },
): Promise<void> {
  if (!qualityEditableFields.some(field => Object.hasOwn(patch, field)))
    throw new Error('请先勾选要修改的字段')
  const current = new Map((await ports.read()).map(rule => [rule.id, rule]))
  for (const id of new Set(ids)) {
    if (ports.stopped())
      break
    const rule = current.get(id)
    if (!rule) {
      ports.onResult({ id, success: false, message: '规则已删除，请检查选择' })
      continue
    }
    try {
      const config = applyQualityPatch(rule.config, patch)
      const changed = JSON.stringify(config) !== JSON.stringify(rule.config)
      if (changed)
        await ports.save({ id, revision: rule.revision, config })
      ports.onResult({ id, success: true, message: changed ? '已保存' : '无需修改' })
    }
    catch (cause) {
      ports.onResult({ id, success: false, message: cause instanceof Error ? cause.message : '保存失败，请重试' })
    }
  }
}
