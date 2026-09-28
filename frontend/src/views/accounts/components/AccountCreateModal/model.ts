import type { AccountModelAccess } from '@/api'
import type { Excel403Action } from '@/utils/excel-settings'
import type { RequestProxySource } from '@/utils/request-proxy-source'
import { normalizeAccountName } from '@/utils/account-name'
import { DEFAULT_EXCEL_MODELS_INPUT } from '@/utils/excel-defaults'
import { excelSettings } from '@/utils/excel-settings'
import { accountModelAccessError } from '../../utils/modelAccess'
import { parseAccountSchedulingForm } from '../../utils/schedulingForm'

export type AccountCreateProvider = 'batch' | 'openai' | 'xai'
export type AccountImportMode = 'oauth' | 'access_token' | 'refresh_token' | 'json' | 'two_fa'
export type AccountImportInputMode = Exclude<AccountImportMode, 'oauth'>

export interface AccountCreateForm {
  customName: string
  provider: AccountCreateProvider | ''
  enabled: boolean
  applyExcel: boolean
  applyRequestProxySource: boolean
  requestProxySource: RequestProxySource
  excelEnabled: boolean
  excelModelsFollowGlobal: boolean
  excelCacheCreationAsInput: boolean
  excel403Action: Excel403Action
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
}

export function emptyAccountCreateForm(): AccountCreateForm {
  return {
    customName: '',
    provider: '',
    enabled: true,
    applyExcel: false,
    applyRequestProxySource: false,
    requestProxySource: 'account',
    excelEnabled: false,
    excelModelsFollowGlobal: true,
    excelCacheCreationAsInput: true,
    excel403Action: 'none',
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
  }
}

export function accountProxyError(form: AccountCreateForm): string | undefined {
  if (form.proxyMode !== 'proxy')
    return undefined
  if (!form.proxyId.trim())
    return '请选择已通过测试的代理'
  return undefined
}

export function accountImportSettings(form: AccountCreateForm, provider = form.provider) {
  const modelError = accountModelAccessError(form.modelAccess)
  if (modelError)
    throw new Error(modelError)
  const customName = normalizeAccountName(form.customName)
  const scheduling = parseAccountSchedulingForm(form.concurrencyLimit, form.weight)
  if (!scheduling.valid)
    throw new Error(scheduling.message)
  return {
    ...(customName ? { customName } : {}),
    enabled: form.enabled,
    ...(form.applyRequestProxySource && provider === 'openai' ? { requestProxySource: form.requestProxySource } : {}),
    ...(form.applyExcel && provider === 'openai' ? excelSettings(form.excelEnabled, form.excelModelsFollowGlobal, form.excelModels, form.excelCacheCreationAsInput, form.excel403Action) : {}),
    ...scheduling.values,
    groupIds: [...new Set(form.groupIds)],
    ...(form.modelAccess ? { modelAccess: { ...form.modelAccess, models: [...form.modelAccess.models] } } : {}),
  }
}
