import type { AccountModelAccess } from '@/api'
import type { AccountTemplate } from '@/api/modules/account-templates'
import type { AccountEgressDraft, AccountEgressPatch } from '@/utils/account-egress'
import type { Excel403Action } from '@/utils/excel-settings'
import type { RequestProxySource } from '@/utils/request-proxy-source'
import { accountEgressPatch, sameAccountEgress } from '@/utils/account-egress'
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
  importTemplate: AccountTemplate | null
  templateEgress?: { draft: AccountEgressDraft, patch: AccountEgressPatch }
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
  excelIgnoreEncryptedContent?: boolean
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
    importTemplate: null,
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
    accountCreateEgress(form, form.provider === 'openai' || form.provider === 'batch')
    return undefined
  }
  catch (error) {
    return error instanceof Error ? error.message : '请选择出站隧道'
  }
}

export function accountCreateEgress(form: AccountCreateForm, openai: boolean) {
  const snapshot = form.templateEgress
  if (snapshot && sameAccountEgress(form, snapshot.draft)) {
    const patch = snapshot.patch
    if (!openai && ((patch.requestProxySource && patch.requestProxySource !== 'account') || (patch.egressMode && patch.egressMode !== 'unchanged')))
      throw new Error('当前账号平台仅支持服务器直连或指定代理')
    return { ...patch, ...(!openai ? { requestProxySource: undefined, egressMode: undefined } : {}) }
  }
  return accountEgressPatch(form, openai)
}

export function accountImportSettings(form: AccountCreateForm, provider = form.provider) {
  const modelError = accountModelAccessError(form.modelAccess)
  if (modelError)
    throw new Error(modelError)
  const customName = normalizeAccountName(form.customName)
  const scheduling = parseAccountSchedulingForm(form.concurrencyLimit, form.weight)
  if (!scheduling.valid)
    throw new Error(scheduling.message)
  const { requestProxySource, egressMode, outboundProxyId } = accountCreateEgress(form, provider === 'openai')
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
          ...(form.excelIgnoreEncryptedContent === undefined ? {} : { excelIgnoreEncryptedContent: form.excelEnabled && form.excelIgnoreEncryptedContent }),
          excelRecovery: excelRecoverySettings(form.excelRecoveryEnabled, form.excelRecoveryInterval),
        }
      : {}),
    ...scheduling.values,
    groupIds: [...new Set(form.groupIds)],
    ...(form.modelAccess ? { modelAccess: { ...form.modelAccess, models: [...form.modelAccess.models] } } : {}),
  }
}

export function accountImportTemplate(form: AccountCreateForm, provider: string) {
  const template = form.importTemplate
  if (!template)
    return undefined
  if (provider !== 'openai' && template.config.turnStateInjectionEnabled === true)
    throw new Error('所选模板的 State 设置仅支持 OpenAI 账号')
  return { id: template.id, revision: template.revision }
}
