import type { CleanupCategory, CleanupConfig } from '@/api/modules/log-cleanup'

export const cleanupCategories: { key: CleanupCategory, label: string, min: number, max: number }[] = [
  { key: 'requests', label: '请求日志（含索引）', min: 31, max: 3650 },
  { key: 'files', label: '运行日志', min: 1, max: 3650 },
  { key: 'captures', label: '错误采集记录', min: 1, max: 30 },
  { key: 'audit', label: '管理员操作日志', min: 1, max: 3650 },
]
export function cleanupBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes) || bytes < 0)
    return '读取失败'
  if (bytes < 1000)
    return `${bytes} B`
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1000)), 4)
  return `${(bytes / (1000 ** index)).toFixed(2)} ${['B', 'KB', 'MB', 'GB', 'TB'][index]}`
}
export function cleanupCutoff(at: string, days: number): string {
  return new Date(new Date(at).getTime() - days * 86400000).toISOString()
}
export function cleanupValidation(config: CleanupConfig): string {
  if (!cleanupCategories.some(({ key }) => config[key].selected))
    return '请选择至少一个清理项目'
  if (cleanupCategories.some(({ key, min, max }) => !Number.isInteger(config[key].retentionDays) || config[key].retentionDays < min || config[key].retentionDays > max))
    return '请检查保留天数'
  return ''
}
