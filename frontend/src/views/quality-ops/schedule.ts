export const scheduleOptions = [
  { value: '1', label: '每小时' },
  { value: '3', label: '每 3 小时' },
  { value: '6', label: '每 6 小时' },
  { value: '12', label: '每 12 小时' },
  { value: 'daily', label: '每天固定时间' },
  { value: 'custom', label: '自定义 Cron（高级）' },
]

export function readSchedule(cron: string): { frequency: string, time: string } {
  const fields = cron.trim().split(/\s+/)
  if (fields.length !== 5 || fields.slice(2).join(' ') !== '* * *')
    return { frequency: 'custom', time: '09:00' }
  if (fields[0] === '0') {
    if (fields[1] === '*')
      return { frequency: '1', time: '09:00' }
    const interval = fields[1]?.match(/^\*\/(12|[136])$/)?.[1]
    if (interval)
      return { frequency: interval, time: '09:00' }
  }
  if (/^\d{1,2}$/.test(fields[0]!) && /^\d{1,2}$/.test(fields[1]!)) {
    const minute = Number(fields[0])
    const hour = Number(fields[1])
    if (minute < 60 && hour < 24)
      return { frequency: 'daily', time: `${String(hour).padStart(2, '0')}:${String(minute).padStart(2, '0')}` }
  }
  return { frequency: 'custom', time: '09:00' }
}

export function writeSchedule(frequency: string, time: string): string | null {
  if (frequency === 'daily') {
    if (!/^(?:[01]\d|2[0-3]):[0-5]\d$/.test(time))
      return null
    const [hour, minute] = time.split(':').map(Number)
    return `${minute} ${hour} * * *`
  }
  if (['1', '3', '6', '12'].includes(frequency))
    return frequency === '1' ? '0 * * * *' : `0 */${frequency} * * *`
  return null
}

export function scheduleSummary(frequency: string, time: string): string {
  if (frequency === 'daily')
    return time ? `每天 ${time}` : ''
  if (frequency === '1')
    return '每小时整点'
  if (['3', '6', '12'].includes(frequency)) {
    const hours = Array.from({ length: 24 / Number(frequency) }, (_, i) => `${String(i * Number(frequency)).padStart(2, '0')}:00`)
    return `每天 ${hours.join('、')}`
  }
  return ''
}
