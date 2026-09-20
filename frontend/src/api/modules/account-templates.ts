import type { RequestOptions } from '../request'
import request from '../request'

export interface AccountTemplateConfig {
  name: string
  enabled: boolean
  turnStateInjectionEnabled?: boolean | null
  concurrencyLimit: number | null
  weight: number
  groupIds: string[]
  outboundProxyId: string | null
}
export interface AccountTemplateSelection { id: string, revision: number }
export interface AccountTemplate extends AccountTemplateSelection { config: AccountTemplateConfig }

// Keep the existing catalog endpoints and records shared with relogin.
export function getAccountTemplates(options: RequestOptions = {}) {
  return request<AccountTemplate[]>({ url: '/api/admin/relogin/templates', method: 'GET', ...options })
}
export function saveAccountTemplate(config: AccountTemplateConfig, selection?: AccountTemplateSelection) {
  return request<AccountTemplate>({ url: '/api/admin/relogin/templates/save', method: 'POST', data: { config, selection } })
}
export function deleteAccountTemplate(selection: AccountTemplateSelection) {
  return request<void>({ url: '/api/admin/relogin/templates/delete', method: 'POST', data: selection })
}
export function applyAccountTemplate(accountIds: string[], template: AccountTemplateSelection) {
  return request<{ accountIds: string[], configRevision: number }>({
    url: '/api/admin/accounts/apply-template',
    method: 'POST',
    data: { accountIds: [...accountIds], template: { id: template.id, revision: template.revision } },
    timeout: 120000,
  })
}
