import type { AccountTemplateConfig } from '@/api/modules/account-templates'
import type { Excel403Action } from '@/utils/excel-settings'
import { accountEgressPatch, accountIpv6Modes } from '@/utils/account-egress'
import { DEFAULT_EXCEL_MODELS } from '@/utils/excel-defaults'
import { accountExcel403Action, excelSettings } from '@/utils/excel-settings'
import { parseAccountSchedulingForm } from '@/views/accounts/utils/schedulingForm'

export function templateForm(config?: AccountTemplateConfig) {
  const proxyMode = config?.requestProxySource && config.requestProxySource !== 'account'
    ? config.requestProxySource
    : config?.outboundProxyId
      ? 'proxy'
      : config?.egressMode && accountIpv6Modes.includes(config.egressMode)
        ? 'ipv6'
        : config?.egressMode === 'unchanged'
          ? 'direct'
          : config?.egressMode === null
            ? 'inherit'
            : config && !config.preserveOutboundProxy ? 'legacy' : 'preserve'
  const proxyId = config?.outboundProxyId ?? ''
  const egressMode = config?.egressMode ?? 'fixed_ipv6_reuse'
  return {
    // Preserve partial settings from pre-unification templates until the user
    // explicitly changes the outbound selection; absence is not inheritance.
    initialEgress: JSON.stringify([proxyMode, proxyId, egressMode]),
    originalEgress: config
      ? {
          ...(config.requestProxySource == null ? {} : { requestProxySource: config.requestProxySource }),
          ...(config.egressMode === undefined ? {} : { egressMode: config.egressMode }),
          ...(config.preserveOutboundProxy === undefined ? {} : { preserveOutboundProxy: config.preserveOutboundProxy }),
          outboundProxyId: config.outboundProxyId ?? null,
        }
      : undefined,
    name: config?.name ?? '',
    enabled: config?.enabled ?? true,
    applyExcel: config === undefined || config.responsesUpstream != null || config.excelModelsFollowGlobal != null || config.excelModels != null,
    excelEnabled: config?.responsesUpstream === 'excel',
    requestProxySource: config?.requestProxySource ?? 'account',
    excelCacheCreationAsInput: config ? config.excelCacheCreationAsInput ?? false : true,
    excelIgnoreEncryptedContent: config?.excelIgnoreEncryptedContent ?? false,
    excel403Action: config ? accountExcel403Action(config) : 'none' as Excel403Action,
    excelModelsFollowGlobal: config?.excelModelsFollowGlobal ?? config?.excelModels == null,
    excelModels: (config?.excelModels ?? DEFAULT_EXCEL_MODELS).join(', '),
    concurrencyLimit: config?.concurrencyLimit == null ? '' : String(config.concurrencyLimit),
    weight: String(config?.weight ?? 1),
    groupIds: [...(config?.groupIds ?? [])],
    proxyMode,
    proxyId,
    egressMode,
  }
}

export function templateConfig(form: ReturnType<typeof templateForm>): AccountTemplateConfig {
  const name = form.name.trim()
  if (!name || new TextEncoder().encode(name).length > 128 || [...name].some(char => char.charCodeAt(0) < 32 || char.charCodeAt(0) === 127))
    throw new Error('模板名称不能为空且不能超过 128 字节')
  const scheduling = parseAccountSchedulingForm(form.concurrencyLimit, form.weight)
  if (!scheduling.valid)
    throw new Error(scheduling.message)
  const unchangedEgress = form.originalEgress && form.initialEgress === JSON.stringify([form.proxyMode, form.proxyId, form.egressMode]) ? form.originalEgress : undefined
  const patch = unchangedEgress ? {} : accountEgressPatch(form, true)
  const egress = unchangedEgress ?? {
    ...patch,
    preserveOutboundProxy: patch.outboundProxyId === undefined,
    outboundProxyId: patch.outboundProxyId || null,
  }
  return {
    name,
    enabled: form.enabled,
    ...(form.applyExcel
      ? {
          ...excelSettings(form.excelEnabled, form.excelModelsFollowGlobal, form.excelModels, form.excelCacheCreationAsInput, form.excel403Action),
          excelIgnoreEncryptedContent: form.excelEnabled && form.excelIgnoreEncryptedContent,
        }
      : {}),
    ...scheduling.values,
    groupIds: [...new Set(form.groupIds)],
    ...egress,
  }
}
