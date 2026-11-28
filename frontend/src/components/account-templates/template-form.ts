import type { AccountTemplateConfig } from '@/api/modules/account-templates'
import { accountEgressPatch, accountIpv6Modes } from '@/utils/account-egress'
import { accountModelAccessError } from '@/views/accounts/utils/modelAccess'
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
    credentialProxyId: config?.credentialProxyId,
    modelAccess: config?.modelAccess ? { ...config.modelAccess, models: [...config.modelAccess.models] } : undefined,
    enabled: config?.enabled ?? true,
    requestProxySource: config?.requestProxySource ?? 'account',
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
  const modelError = accountModelAccessError(form.modelAccess)
  if (modelError)
    throw new Error(modelError)
  const unchangedEgress = form.originalEgress && form.initialEgress === JSON.stringify([form.proxyMode, form.proxyId, form.egressMode]) ? form.originalEgress : undefined
  const patch = unchangedEgress ? {} : accountEgressPatch(form, true)
  const egress = unchangedEgress ?? {
    ...patch,
    preserveOutboundProxy: patch.outboundProxyId === undefined,
    outboundProxyId: patch.outboundProxyId || null,
  }
  return {
    name,
    ...(form.credentialProxyId === undefined ? {} : { credentialProxyId: form.credentialProxyId }),
    ...(form.modelAccess ? { modelAccess: { ...form.modelAccess, models: [...form.modelAccess.models] } } : {}),
    enabled: form.enabled,
    ...scheduling.values,
    groupIds: [...new Set(form.groupIds)],
    ...egress,
  }
}
