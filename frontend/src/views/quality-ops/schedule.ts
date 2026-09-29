import type { QualityRuleConfig } from '@/api/modules/quality-ops'

export const DEFAULT_QUALITY_INTERVAL_SECONDS = 60
export const MIN_QUALITY_INTERVAL_SECONDS = 5
export const MAX_QUALITY_INTERVAL_SECONDS = 31_536_000

export function qualityScheduleSummary(config: Pick<QualityRuleConfig, 'intervalSeconds' | 'cron' | 'timezone'>): string {
  return config.intervalSeconds != null
    ? `每 ${config.intervalSeconds} 秒`
    : `原定时：${config.cron}（${config.timezone}）`
}
