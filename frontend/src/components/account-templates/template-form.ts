import type { AccountTemplateConfig } from '@/api/modules/account-templates'
import { excelSettings } from '@/utils/excel-settings'
import { parseAccountSchedulingForm } from '@/views/accounts/utils/schedulingForm'

export function templateForm(config?: AccountTemplateConfig) {
  return {
    name: config?.name ?? '',
    enabled: config?.enabled ?? true,
    applyExcel: config === undefined || config.responsesUpstream != null || config.excelModelsFollowGlobal != null || config.excelModels != null,
    excelEnabled: config?.responsesUpstream === 'excel',
    excelCacheCreationAsInput: config?.excelCacheCreationAsInput ?? false,
    excelAutoDisableOn403: config?.excelAutoDisableOn403 ?? false,
    excelModelsFollowGlobal: config?.excelModelsFollowGlobal ?? config?.excelModels == null,
    excelModels: (config?.excelModels ?? ['gpt-5.6-sol', 'gpt-6-astra']).join(', '),
    concurrencyLimit: config?.concurrencyLimit == null ? '' : String(config.concurrencyLimit),
    weight: String(config?.weight ?? 1),
    groupIds: [...(config?.groupIds ?? [])],
    proxyMode: config?.outboundProxyId ? 'proxy' : 'direct',
    proxyId: config?.outboundProxyId ?? '',
  }
}

export function templateConfig(form: ReturnType<typeof templateForm>): AccountTemplateConfig {
  const name = form.name.trim()
  if (!name || new TextEncoder().encode(name).length > 128 || [...name].some(char => char.charCodeAt(0) < 32 || char.charCodeAt(0) === 127))
    throw new Error('模板名称不能为空且不能超过 128 字节')
  const scheduling = parseAccountSchedulingForm(form.concurrencyLimit, form.weight)
  if (!scheduling.valid)
    throw new Error(scheduling.message)
  if (form.proxyMode === 'proxy' && !form.proxyId)
    throw new Error('请选择已通过测试的代理')
  return {
    name,
    enabled: form.enabled,
    ...(form.applyExcel ? excelSettings(form.excelEnabled, form.excelModelsFollowGlobal, form.excelModels, form.excelCacheCreationAsInput, form.excelAutoDisableOn403) : {}),
    ...scheduling.values,
    groupIds: [...new Set(form.groupIds)],
    outboundProxyId: form.proxyMode === 'proxy' ? form.proxyId : null,
  }
}
