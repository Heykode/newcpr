import type { MaintenanceAction, ReloginEntry } from '@/api/modules/relogin'
import { processingStatus } from './presentation'

export const maintenanceLabels: Record<MaintenanceAction, string> = {
  edit_material: '修改本地登录资料',
  check_session: '检测会话',
  devices: '设备与会话',
  workspaces: '工作区',
  security: '安全状态',
  revoke_device: '退出设备',
  logout_all: '退出全部设备',
  logout_and_relogin: '退出并重登',
  authorize: '重新授权',
  force_login: '密码与 2FA 重登',
  switch_workspace: '切换并授权',
  prepare_totp: '准备新 2FA',
  activate_totp: '激活新 2FA',
  disable_totp: '停用旧 2FA',
}
export function sessionLabel(row: ReloginEntry) {
  return ({ valid: '有效', expired: '已失效', absent: '未建立' } as Record<string, string>)[row.maintenance?.sessionState ?? ''] ?? '未检测'
}
export function maintenanceBusy(row: ReloginEntry) {
  return row.maintenance?.state === 'running' || ['queued', 'running', 'pushing'].includes(row.status)
}
export function recentOperationAt(row: ReloginEntry) {
  return [row.lastAttemptAt || row.lastReloginAt, row.maintenance?.checkedAt, row.lastActivity?.startedAt]
    .filter((value): value is string => !!value)
    .sort((left, right) => Date.parse(right) - Date.parse(left))[0]
}
export function recentOperation(row: ReloginEntry) {
  if (row.lastActivity?.operation && recentOperationAt(row) === row.lastActivity.startedAt)
    return maintenanceLabels[row.lastActivity.operation]
  const maintenance = row.maintenance
  if (maintenance?.action && maintenance.checkedAt && recentOperationAt(row) === maintenance.checkedAt)
    return maintenanceLabels[maintenance.action]
  return row.lastAttemptAt ? '获取凭据' : '—'
}

export function matchesMaintenanceStatus(row: ReloginEntry, status: string) {
  if (!status)
    return true
  if (status === 'active')
    return row.maintenance?.state === 'running' || ['running', 'pushing'].includes(row.status)
  if (status === 'needs_attention')
    return ['pending', 'awaiting_workspace'].includes(row.status) || Boolean(row.maintenance?.uncertain || row.maintenance?.pendingTotp || row.maintenance?.loginPaused)
  if (status === 'failed')
    return row.status === 'failed' || row.maintenance?.state === 'failed'
  return processingStatus(row).key === status
}

export function maintenanceResult(row: ReloginEntry) {
  if (row.lastActivity && recentOperationAt(row) === row.lastActivity.startedAt)
    return { label: row.lastActivity.result === 'succeeded' ? (row.lastActivity.operation === 'edit_material' ? '已保存' : '已完成') : ({ running: '执行中', cancelled: '已取消', awaiting_workspace: '待选择工作区' } as Record<string, string>)[row.lastActivity.result] ?? '失败', tone: row.lastActivity.result === 'succeeded' ? 'text-cp-success' : 'text-cp-error' }
  if (row.maintenance?.uncertain)
    return { label: '结果待核实', tone: 'text-cp-warning' }
  const maintenance = row.maintenance
  if (maintenance?.checkedAt && recentOperationAt(row) === maintenance.checkedAt) {
    const label = ({ running: '执行中', succeeded: '已完成', failed: '失败', cancelled: '已取消', awaiting_workspace: '待选择工作区' } as Record<string, string>)[maintenance.state]
    if (label)
      return { label, tone: maintenance.state === 'failed' ? 'text-cp-error' : 'text-cp-text-secondary' }
  }
  return processingStatus(row)
}

export function egressAddress(row: ReloginEntry) {
  const egress = row.egress
  if (!egress)
    return '未读取'
  if (egress.kind === 'default')
    return '部署默认出口'
  if (egress.kind === 'unavailable' || egress.kind === 'multiple')
    return egress.kind === 'multiple' ? '多个工作区出口' : '出口信息不可用'
  return egress.exitIp || egress.address || egress.label
}
export function egressLabel(row: ReloginEntry) {
  const egress = row.egress
  if (!egress)
    return '刷新后同步账号配置'
  if (egress.kind === 'proxy')
    return `${egress.label} · ${egress.exitIp ? '最近检测 IP' : '代理地址'}`
  return egress.kind === 'default' ? (egress.source === 'account' ? '跟随账号配置' : '未绑定独立出口') : egress.label
}
