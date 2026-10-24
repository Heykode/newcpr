import type { RelativeTimeView } from '@/api/time'
import { t } from '@/i18n'

export function formatRelativeTime(value: RelativeTimeView | null | undefined, fallback: string): string {
  if (!value)
    return fallback
  if (value.kind === 'absolute')
    return value.value
  if (value.kind === 'just_now')
    return t('common.time.justNow')
  const keys = { minute: 'common.time.minutesAgo', hour: 'common.time.hoursAgo', day: 'common.time.daysAgo' }
  return t(keys[value.unit], { count: value.value })
}
