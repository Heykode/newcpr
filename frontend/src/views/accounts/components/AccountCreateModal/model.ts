import type { AccountModelAccess } from '@/api'
import type { Excel403Action } from '@/utils/excel-settings'
import type { RequestProxySource } from '@/utils/request-proxy-source'
import { accountEgressPatch } from '@/utils/account-egress'
import { normalizeAccountName } from '@/utils/account-name'
import { DEFAULT_EXCEL_MODELS_INPUT } from '@/utils/excel-defaults'
import { excelRecoverySettings, excelSettings } from '@/utils/excel-settings'
import { accountModelAccessError } from '../../utils/modelAccess'
import { purchaseCostPatch } from '../../utils/purchaseCost'
import { parseAccountSchedulingForm } from '../../utils/schedulingForm'

export type AccountCreateProvider = 'batch' | 'openai' | 'xai'
export type AccountImportMode = 'oauth' | 'access_token' | 'refresh_token' | 'json' | 'two_fa'
export type AccountImportInputMode = Exclude<AccountImportMode, 'oauth'>

export interface AccountCreateForm {
  purchaseAmount: string
  purchaseCycleStart: string
  customName: string
  provider: AccountCreateProvider | ''
  enabled: boolean
  applyExcel: boolean
  requestProxySource: RequestProxySource
  excelEnabled: boolean
  excelModelsFollowGlobal: boolean
  excelCacheCreationAsInput: boolean
  excel403Action: Excel403Action
  excelRecoveryEnabled: boolean
  excelRecoveryInterval: string
  excelModels: string
  concurrencyLimit: string
  weight: string
  modelAccess?: AccountModelAccess
  groupIds: string[]
  step: 'settings' | 'import'
  mode: AccountImportMode
  importTexts: Record<AccountImportInputMode, string>
  replaceExisting2fa: boolean
  oauthFlowId: string
  oauthAuthUrl: string
  oauthCallback: string
  proxyMode: string
  proxyId: string
  egressMode: string
}

export function emptyAccountCreateForm(): AccountCreateForm {
  return {
    purchaseAmount: '',
    purchaseCycleStart: '',
    customName: '',
    provider: '',
    enabled: true,
    applyExcel: false,
    requestProxySource: 'account',
    excelEnabled: false,
    excelModelsFollowGlobal: true,
    excelCacheCreationAsInput: true,
    excel403Action: 'none',
    excelRecoveryEnabled: false,
    excelRecoveryInterval: '60',
    excelModels: DEFAULT_EXCEL_MODELS_INPUT,
    concurrencyLimit: '',
    weight: '1',
    groupIds: [],
    step: 'settings',
    mode: 'oauth',
    importTexts: { access_token: '', refresh_token: '', json: '', two_fa: '' },
    replaceExisting2fa: false,
    oauthFlowId: '',
    oauthAuthUrl: '',
    oauthCallback: '',
    proxyMode: 'direct',
    proxyId: '',
    egressMode: 'fixed_ipv6_reuse',
  }
}

export function accountProxyError(form: AccountCreateForm): string | undefined {
  try {
    accountEgressPatch(form, form.provider === 'openai')
    return undefined
  }
  catch (error) {
    return error instanceof Error ? error.message : '请选择出站隧道'
  }
}

export function accountImportSettings(form: AccountCreateForm, provider = form.provider) {
  const modelError = accountModelAccessError(form.modelAccess)
  if (modelError)
    throw new Error(modelError)
  const customName = normalizeAccountName(form.customName)
  const scheduling = parseAccountSchedulingForm(form.concurrencyLimit, form.weight)
  if (!scheduling.valid)
    throw new Error(scheduling.message)
  const { requestProxySource, egressMode, outboundProxyId } = accountEgressPatch(form, provider === 'openai')
  return {
    ...(outboundProxyId === '' ? { clearOutboundProxy: true } : {}),
    ...(form.purchaseAmount.trim() ? { purchaseCost: purchaseCostPatch(form.purchaseAmount, form.purchaseCycleStart) } : {}),
    ...(customName ? { customName } : {}),
    enabled: form.enabled,
    ...(requestProxySource === undefined ? {} : { requestProxySource }),
    ...(egressMode === undefined ? {} : { egressMode }),
    ...(form.applyExcel && provider === 'openai'
      ? {
          ...excelSettings(form.excelEnabled, form.excelModelsFollowGlobal, form.excelModels, form.excelCacheCreationAsInput, form.excel403Action),
          excelRecovery: excelRecoverySettings(form.excelRecoveryEnabled, form.excelRecoveryInterval),
        }
      : {}),
    ...scheduling.values,
    groupIds: [...new Set(form.groupIds)],
    ...(form.modelAccess ? { modelAccess: { ...form.modelAccess, models: [...form.modelAccess.models] } } : {}),
  }
}
