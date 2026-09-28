import type { AccountTemplateConfig } from '@/api/modules/account-templates'
import type { Excel403Action } from '@/utils/excel-settings'
import { DEFAULT_EXCEL_MODELS } from '@/utils/excel-defaults'
import { accountExcel403Action, excelSettings } from '@/utils/excel-settings'
import { parseAccountSchedulingForm } from '@/views/accounts/utils/schedulingForm'

export function templateForm(config?: AccountTemplateConfig) {
  return {
    name: config?.name ?? '',
    enabled: config?.enabled ?? true,
    applyExcel: config === undefined || config.responsesUpstream != null || config.excelModelsFollowGlobal != null || config.excelModels != null,
    excelEnabled: config?.responsesUpstream === 'excel',
    excelCacheCreationAsInput: config ? config.excelCacheCreationAsInput ?? false : true,
    excel403Action: config ? accountExcel403Action(config) : 'none' as Excel403Action,
    excelModelsFollowGlobal: config?.excelModelsFollowGlobal ?? config?.excelModels == null,
    excelModels: (config?.excelModels ?? DEFAULT_EXCEL_MODELS).join(', '),
    concurrencyLimit: config?.concurrencyLimit == null ? '' : String(config.concurrencyLimit),
    weight: String(config?.weight ?? 1),
    groupIds: [...(config?.groupIds ?? [])],
    proxyMode: config?.outboundProxyId ? 'proxy' : 'direct',
    proxyId: config?.outboundProxyId ?? '',
    egressMode: config?.egressMode === undefined ? 'preserve' : config.egressMode ?? 'inherit',
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
  if (form.proxyMode === 'proxy' && !['preserve', 'inherit', 'unchanged'].includes(form.egressMode))
    throw new Error('IPv6 策略不能与账号代理同时启用，请将出站代理改为直连')
  return {
    name,
    enabled: form.enabled,
    ...(form.applyExcel ? excelSettings(form.excelEnabled, form.excelModelsFollowGlobal, form.excelModels, form.excelCacheCreationAsInput, form.excel403Action) : {}),
    ...scheduling.values,
    groupIds: [...new Set(form.groupIds)],
    outboundProxyId: form.proxyMode === 'proxy' ? form.proxyId : null,
    ...(form.egressMode === 'preserve' ? {} : { egressMode: form.egressMode === 'inherit' ? null : form.egressMode }),
  }
}
