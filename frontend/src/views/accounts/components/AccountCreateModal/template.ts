import type { AccountCreateForm } from './model'
import type { AccountTemplate } from '@/api/modules/account-templates'
import { templateForm } from '@/components/account-templates/template-form'

export function prefillAccountTemplate(form: AccountCreateForm, selected: AccountTemplate | null): AccountCreateForm {
  if (!selected)
    return { ...form, importTemplate: null }
  const template = {
    ...selected,
    config: {
      ...selected.config,
      groupIds: [...selected.config.groupIds],
      ...(selected.config.modelAccess ? { modelAccess: { ...selected.config.modelAccess, models: [...selected.config.modelAccess.models] } } : {}),
    },
  }
  const config = template.config
  const next = { ...form, importTemplate: template }
  if (config.credentialProxyId !== undefined)
    next.credentialProxyId = config.credentialProxyId
  next.enabled = config.enabled
  next.concurrencyLimit = config.concurrencyLimit == null ? '' : String(config.concurrencyLimit)
  next.weight = String(config.weight)
  next.groupIds = [...config.groupIds]
  if (config.modelAccess != null)
    next.modelAccess = { ...config.modelAccess, models: [...config.modelAccess.models] }
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
