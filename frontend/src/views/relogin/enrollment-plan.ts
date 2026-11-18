import type { AccountCreateForm } from '../accounts/components/AccountCreateModal/model'
import type { ReloginEntry } from '@/api/modules/relogin'

// Only the isolated Vite fixture defines this flag. The production API has no plan executor.
export const enrollmentPreview = import.meta.env.DEV && import.meta.env.VITE_ENROLLMENT_PREVIEW === '1'

const settingKeys = [
  'customName',
  'importTemplate',
  'qualityTemplate',
  'templateEgress',
  'enabled',
  'groupIds',
  'modelAccess',
  'proxyMode',
  'proxyId',
  'credentialProxyId',
  'egressMode',
  'requestProxySource',
  'weight',
  'concurrencyLimit',
  'purchaseAmount',
  'purchaseCycleStart',
  'applyExcel',
  'excelEnabled',
  'excelModelsFollowGlobal',
  'excelCacheCreationAsInput',
  'excelIgnoreEncryptedContent',
  'excel403Action',
  'excelRecoveryEnabled',
  'excelRecoveryInterval',
  'excelModels',
] as const

export function enrollmentSettings(form: AccountCreateForm) {
  return Object.fromEntries(settingKeys.map(key => [key, form[key]])) as Pick<AccountCreateForm, typeof settingKeys[number]>
}

export interface EnrollmentPlan {
  state: 'draft' | 'scheduled' | 'running' | 'failed' | 'verification' | 'done'
  settings: ReturnType<typeof enrollmentSettings>
  when: string | null
  exitLabel: string
  credentialExitLabel?: string
}

export type EnrollmentEntry = ReloginEntry & { enrollmentPlan?: EnrollmentPlan, credentialExitLabel?: string }
export function separateCredentialExit(row: EnrollmentEntry) {
  if (!enrollmentPreview)
    return undefined
  const label = canArrange(row) ? row.enrollmentPlan?.credentialExitLabel : row.credentialExitLabel
  return label !== (canArrange(row) ? row.enrollmentPlan?.exitLabel : row.egress?.label) ? label : undefined
}
export const enrollmentLabels = { draft: '已存草稿', scheduled: '已定时', running: '入池中', failed: '出口不可用', verification: '待人工验证', done: '已入池' }

export function canArrange(row: EnrollmentEntry) {
  return enrollmentPreview && row.poolAccountIds.length === 0 && row.poolStatus === 'absent'
}

export async function previewPlanCommand(body: Record<string, unknown>) {
  if (!enrollmentPreview)
    throw new Error('本地预览未开启')
  const result = await fetch('/__preview__/enrollment', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'omit',
    redirect: 'error',
    body: JSON.stringify(body),
  })
  const data = await result.json()
  if (!result.ok)
    throw new Error(data.message || '操作失败，请刷新列表')
}
