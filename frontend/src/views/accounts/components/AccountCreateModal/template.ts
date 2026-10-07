import type { AccountCreateForm } from './model'
import type { AccountTemplate } from '@/api/modules/account-templates'
import { templateForm } from '@/components/account-templates/template-form'
import { accountExcel403Action } from '@/utils/excel-settings'

export function prefillAccountTemplate(form: AccountCreateForm, selected: AccountTemplate | null): AccountCreateForm {
  if (!selected)
    return { ...form, importTemplate: null }
  const template = {
    ...selected,
    config: {
      ...selected.config,
      groupIds: [...selected.config.groupIds],
      ...(selected.config.modelAccess ? { modelAccess: { ...selected.config.modelAccess, models: [...selected.config.modelAccess.models] } } : {}),
      ...(selected.config.excelModels ? { excelModels: [...selected.config.excelModels] } : {}),
      ...(selected.config.excelRecovery ? { excelRecovery: { ...selected.config.excelRecovery } } : {}),
    },
  }
  const config = template.config
  const next = { ...form, importTemplate: template }
  next.enabled = config.enabled
  next.concurrencyLimit = config.concurrencyLimit == null ? '' : String(config.concurrencyLimit)
  next.weight = String(config.weight)
  next.groupIds = [...config.groupIds]
  if (config.modelAccess != null)
    next.modelAccess = { ...config.modelAccess, models: [...config.modelAccess.models] }
  if (config.responsesUpstream != null)
    next.excelEnabled = config.responsesUpstream === 'excel'
  if (config.excelModelsFollowGlobal != null)
    next.excelModelsFollowGlobal = config.excelModelsFollowGlobal
  else if (config.excelModels != null)
    next.excelModelsFollowGlobal = false
  if (config.excelModels != null)
    next.excelModels = config.excelModels.join(', ')
  if (config.excelCacheCreationAsInput != null)
    next.excelCacheCreationAsInput = config.excelCacheCreationAsInput
  if (config.excelIgnoreEncryptedContent != null)
    next.excelIgnoreEncryptedContent = config.excelIgnoreEncryptedContent
  if (config.excel403Action != null || config.excelAutoDisableOn403 != null)
    next.excel403Action = accountExcel403Action(config)
  if (config.excelRecovery != null) {
    next.excelRecoveryEnabled = config.excelRecovery.enabled
    next.excelRecoveryInterval = String(config.excelRecovery.intervalMinutes)
  }
  if ([config.responsesUpstream, config.excelModelsFollowGlobal, config.excelModels, config.excelCacheCreationAsInput, config.excelIgnoreEncryptedContent, config.excel403Action, config.excelAutoDisableOn403, config.excelRecovery].some(value => value != null))
    next.applyExcel = true

  // Legacy templates can leave IPv6/source untouched while replacing only the proxy.
  // Keep that partial patch until the administrator changes the visible exit.
  const egress = templateForm(config)
  const draft = { proxyMode: egress.proxyMode, proxyId: egress.proxyId, egressMode: egress.egressMode }
  Object.assign(next, draft)
  next.templateEgress = {
    draft,
    patch: {
      ...(config.requestProxySource == null ? {} : { requestProxySource: config.requestProxySource }),
      ...(config.egressMode === undefined ? {} : { egressMode: config.egressMode }),
      ...(!config.preserveOutboundProxy ? { outboundProxyId: config.outboundProxyId ?? '' } : {}),
    },
  }
  return next
}
