import type { AccountModelAccess } from '@/api'
import { normalizeAccountName } from '@/utils/account-name'
import { DEFAULT_EXCEL_MODELS_INPUT } from '@/utils/excel-defaults'
import { excelSettings } from '@/utils/excel-settings'
import { accountModelAccessError } from '../../utils/modelAccess'
import { parseAccountSchedulingForm } from '../../utils/schedulingForm'

export type AccountCreateProvider = 'batch' | 'openai' | 'xai'
export type AccountImportMode = 'oauth' | 'access_token' | 'refresh_token' | 'json'
export type AccountImportInputMode = Exclude<AccountImportMode, 'oauth'>

export interface AccountCreateForm {
  customName: string
  provider: AccountCreateProvider | ''
  enabled: boolean
  applyExcel: boolean
  excelEnabled: boolean
  excelModelsFollowGlobal: boolean
  excelCacheCreationAsInput: boolean
  excelAutoDisableOn403: boolean
  excelModels: string
  concurrencyLimit: string
  weight: string
  modelAccess?: AccountModelAccess
  groupIds: string[]
  step: 'settings' | 'import'
  mode: AccountImportMode
  importTexts: Record<AccountImportInputMode, string>
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
    excelEnabled: false,
    excelModelsFollowGlobal: true,
    excelCacheCreationAsInput: false,
    excelAutoDisableOn403: false,
    excelModels: DEFAULT_EXCEL_MODELS_INPUT,
    concurrencyLimit: '',
    weight: '1',
    groupIds: [],
    step: 'settings',
    mode: 'oauth',
    importTexts: { access_token: '', refresh_token: '', json: '' },
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
    ...(form.applyExcel && provider === 'openai' ? excelSettings(form.excelEnabled, form.excelModelsFollowGlobal, form.excelModels, form.excelCacheCreationAsInput, form.excelAutoDisableOn403) : {}),
    ...scheduling.values,
    groupIds: [...new Set(form.groupIds)],
    ...(form.modelAccess ? { modelAccess: { ...form.modelAccess, models: [...form.modelAccess.models] } } : {}),
  }
}
