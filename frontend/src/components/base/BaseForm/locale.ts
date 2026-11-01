import { computed } from 'vue'
import { t } from '@/i18n'

export function useLocale() {
  return computed(() => ({ form: {
    required: t('common.form.required'),
    requiredMessage: t('common.form.requiredMessage'),
    invalidMessage: t('common.form.invalidMessage'),
  } }))
}
