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
    'requestTuning.keyConcurrencyWaitTimeoutSeconds': integer(1, 600),
    'requestTuning.maxWaitingPerAccount': integer(1, 1000),
    'requestTuning.accountConcurrencyWaitTimeoutSeconds': integer(1, 600),
    'requestTuning.websocketMaxRetries': integer(0, 100),
    'requestTuning.maxAccountSwitches': integer(0, 31),
    'requestTuning.maxRequestAttempts': integer(1, 32),
    'accountWarmupScheduleTime': {
      required: true,
      pattern: /^(?:[01]\d|2[0-3]):[0-5]\d(?:,(?:[01]\d|2[0-3]):[0-5]\d)*$/,
      transform: value => value.trim(),
      trigger: ['blur', 'change'],
      message: '时间须为 HH:MM，多个时间用逗号分隔',
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
      trigger: 'change',
      validator: () => {
        const error = protectionRulesError(form.accountErrorProtection)
        return error ? new Error(error) : true
      },
    },
  }))
}
