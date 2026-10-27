import type { ApiKeyFormValue } from '../composables/useApiKeyMutations'

export function apiKeyFormErrors(form: ApiKeyFormValue) {
  const errors: Partial<Record<keyof ApiKeyFormValue, string>> = {}
  if (!form.name.trim())
    errors.name = '请输入 API Key 名称'
  for (const [key, label] of [['dailyLimitUsd', '日限额'], ['weeklyLimitUsd', '周限额']] as const) {
    const value = form[key].trim()
    if (value && !/^\d{1,10}(?:\.\d{1,10})?$/.test(value))
      errors[key] = `${label}必须是非负金额，最多 10 位小数`
  }
  for (const [key, label] of [['maxConcurrency', '最大并发'], ['requestsPerMinute', '每分钟请求数']] as const) {
    const value = Number(form[key])
    if (!Number.isSafeInteger(value) || value < 0)
      errors[key] = `${label}必须是非负整数`
  }
  return errors
}
