import type { QualityModelChoice } from '@/api/modules/quality-ops'

export const qualityEfforts = ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']

export function mergeModelChoices(previous: QualityModelChoice[], incoming: QualityModelChoice[]) {
  return [...new Map([...previous, ...incoming].map(model => [model.id, model])).values()]
    .sort((a, b) => a.id.localeCompare(b.id))
}

export function qualityEffortOptions(models: QualityModelChoice[], model: string, current: string) {
  const supported = models.find(choice => choice.id === model)?.reasoningEfforts
  const values = [...new Set([...(supported ?? []), ...qualityEfforts, ...(current ? [current] : [])])]
  return [{ value: '', label: '按默认' }, ...values.map(value => ({
    value,
    label: supported?.includes(value) ? `${value}（目录支持）` : `${value}（自定义）`,
  }))]
}
