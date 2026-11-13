import { computed } from 'vue'
import { locale } from '@/i18n'
import en from './locales/en'
import zhHans from './locales/zh-Hans'
import zhHant from './locales/zh-Hant'

export function useLocale() {
  return computed(() => locale.value === 'en' ? en : locale.value === 'zh-Hant' ? zhHant : zhHans)
}
