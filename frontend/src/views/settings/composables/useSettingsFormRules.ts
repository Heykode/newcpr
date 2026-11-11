import type { useSettingsForm } from './useSettingsForm'
import type { FormRule, FormRules } from '@/components/base/BaseForm/types'
import { computed } from 'vue'
import { protectionRulesError, trimProtectionText } from '../components/error-protection/form'

type SettingsForm = ReturnType<typeof useSettingsForm>['form']

// Official v3.37.2 field feedback, with the native settings API's units and bounds.
export function useSettingsFormRules(form: SettingsForm) {
  const integer = (min: number, max: number): FormRule => ({
    type: 'integer',
    required: true,
    min,
    max,
    trigger: ['blur', 'change'],
    message: `请输入 ${min}–${max} 的整数`,
  })
  return computed<FormRules>(() => ({
    'refreshMarginSeconds': integer(0, Number.MAX_SAFE_INTEGER),
    'refreshConcurrency': integer(0, 4_294_967_295),
    'maxConcurrentPerAccount': integer(0, 4_294_967_295),
    'openaiGuardianReservedConcurrency': integer(0, 4_294_967_295),
    'openaiSessionBindingTtlHours': integer(1, 720),
    'requestIntervalMs': integer(0, Number.MAX_SAFE_INTEGER),
    'responsesMaxDecompressedBodyBytes': integer(1, 256 * 1024 * 1024),
    'requestTuning.maxWaitingPerKey': integer(0, 1024),
    'requestTuning.keyQueueTimeoutSeconds': integer(1, 120),
    'requestTuning.maxWaitingPerAccount': integer(1, 1000),
    'requestTuning.accountQueueTimeoutSeconds': integer(1, 120),
    'requestTuning.websocketMaxRetries': integer(0, 100),
    'requestTuning.maxAccountSwitches': integer(0, 31),
    'requestTuning.maxRequestAttempts': integer(1, 32),
    'requestTuning.maxRetryAfterSeconds': integer(0, 4_294_967_295),
    'accountWarmupCron': {
      required: form.accountWarmupEnabled,
      pattern: /^\S+(?: +\S+){4}$/,
      max: 255,
      transform: value => value.trim(),
      trigger: ['blur', 'change'],
      message: '请填写五段 Cron 表达式',
    },
    'accountWarmupModel': {
      required: form.accountWarmupEnabled,
      whitespace: true,
      max: 128,
      transform: value => value.trim(),
      trigger: ['blur', 'change'],
      message: '请填写不超过 128 个字符的预热模型',
    },
    'accountErrorProtection.threshold': integer(2, 1000),
    'accountErrorProtection.windowSeconds': integer(60, 3600),
    'accountErrorProtection.durationSeconds': integer(300, 604800),
    'accountErrorProtection.probeModel': {
      pattern: /^\P{Cc}*$/u,
      max: 128,
      transform: trimProtectionText,
      trigger: ['blur', 'change'],
      message: '探测模型不可包含控制字符，最长 128 个字符',
    },
    'accountErrorProtection.rules': {
      trigger: [],
      validator: () => {
        const error = protectionRulesError(form.accountErrorProtection)
        return error ? new Error(error) : true
      },
    },
  }))
}
