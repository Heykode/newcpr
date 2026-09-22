import type { RequestOptions } from '../request'
import request from '../request'

export type SmtpSecurity = 'none' | 'starttls' | 'tls'
export type BarkLevel = 'passive' | 'active' | 'timeSensitive' | 'critical'

export interface NotificationDeliveryRecord {
  id: string
  groupId: string | null
  channel: 'email' | 'bark'
  target: string
  status: 'pending' | 'sending' | 'sent' | 'failed'
  test: boolean
  attempts: number
  error: string | null
  createdAt: string
  finishedAt: string | null
}

export interface NotificationChannels {
  smtp: {
    enabled: boolean
    host: string
    port: number
    security: SmtpSecurity
    username: string | null
    passwordSet: boolean
    fromName: string | null
    fromEmail: string | null
    password?: string
  }
  bark: {
    enabled: boolean
    serverUrl: string
    deviceKeySet: boolean
    level: BarkLevel
    sound: string | null
    volume: number
    call: boolean
    deviceKey?: string
  }
  lastTest: NotificationDeliveryRecord | null
  updatedAt: string
}

export interface AlertConditionPolicy {
  enabled: boolean
  threshold: number
  confirmationSeconds: number
}

export interface GroupAlertPolicy {
  groupId: string
  enabled: boolean
  concurrency: AlertConditionPolicy
  eta: AlertConditionPolicy
  quotaZero: AlertConditionPolicy
  availability: AlertConditionPolicy
  emailEnabled: boolean
  emailRecipients: string[]
  barkEnabled: boolean
  barkLevel: BarkLevel | null
  barkSound: string | null
  barkVolume: number | null
  barkCall: boolean | null
  updatedAt: string
}

export function getNotificationChannels(options: RequestOptions = {}) {
  return request<NotificationChannels>({ url: '/api/admin/notifications/channels', method: 'GET', ...options })
}

export function updateNotificationChannels(data: NotificationChannels, options: RequestOptions = {}) {
  return request<NotificationChannels>({ url: '/api/admin/notifications/channels/update', method: 'POST', data: { smtp: data.smtp, bark: data.bark }, ...options })
}

export function testNotification(data: { channel: 'email' | 'bark', target: string, groupId?: string | null }, options: RequestOptions = {}) {
  return request<{ id: string }>({ url: '/api/admin/notifications/test', method: 'POST', data, ...options })
}

export function getGroupAlertPolicy(groupId: string, options: RequestOptions = {}) {
  return request<GroupAlertPolicy>({ url: '/api/admin/account-groups/alert-policy', method: 'GET', params: { groupId }, ...options })
}

export function updateGroupAlertPolicy(data: GroupAlertPolicy, options: RequestOptions = {}) {
  const { updatedAt: _updatedAt, ...policy } = data
  return request<GroupAlertPolicy>({ url: '/api/admin/account-groups/alert-policy/update', method: 'POST', data: policy, ...options })
}

export function getNotificationDeliveries(groupId = '', options: RequestOptions = {}) {
  return request<NotificationDeliveryRecord[]>({ url: '/api/admin/notifications/deliveries', method: 'GET', params: { groupId }, ...options })
}
