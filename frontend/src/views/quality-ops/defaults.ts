import type { QualityRuleConfig } from '@/api/modules/quality-ops'
import { CANDY_PROMPT, CANDY_REFERENCE_ANSWER, DEFAULT_JUDGE_PROMPT } from './presets'
import { DEFAULT_QUALITY_INTERVAL_SECONDS } from './schedule'

export function newQualityConfig(timezone: string): QualityRuleConfig {
  return {
    detectionMode: 'state_probe',
    accountId: '',
    model: 'gpt-6-astra',
    enabled: true,
    intervalSeconds: DEFAULT_QUALITY_INTERVAL_SECONDS,
    cron: '',
    timezone,
    repetitions: 1,
    prompt: CANDY_PROMPT,
    referenceAnswer: CANDY_REFERENCE_ANSWER,
    reasoningEffort: null,
    judgeGroupId: '',
    judgeModel: '',
    judgePrompt: DEFAULT_JUDGE_PROMPT,
    // Remediation still requires the administrator's explicit template selection.
    failureAction: 'none',
    failureTemplate: null,
    failureGroupIds: [],
    autoRestore: false,
    disableExcelOnNativeRecovery: true,
    excelFailureThreshold: 2,
    excelRecoveryThreshold: 2,
  }
}

export function qualityConfigDraft(config: Partial<QualityRuleConfig>, timezone: string): QualityRuleConfig {
  return {
    ...newQualityConfig(timezone),
    ...config,
    detectionMode: config.detectionMode ?? 'answer',
    model: config.model ?? '',
    intervalSeconds: config.intervalSeconds ?? null,
    disableExcelOnNativeRecovery: config.disableExcelOnNativeRecovery ?? false,
    excelFailureThreshold: config.excelFailureThreshold ?? 1,
    excelRecoveryThreshold: config.excelRecoveryThreshold ?? 1,
    failureGroupIds: [...(config.failureGroupIds ?? [])],
  }
}
