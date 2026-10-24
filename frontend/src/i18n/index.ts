import type { Locale } from './locale'
import { createI18n } from 'vue-i18n'
import { applyLocale, readInitialLocale } from './locale'

export const i18n = createI18n({
  legacy: false,
  locale: readInitialLocale(),
  fallbackLocale: 'zh-Hans',
  messages: {},
})

export const t: typeof i18n.global.t = i18n.global.t
export const { locale } = i18n.global
const loaders = {
  'en': () => import('./locales/en'),
  'zh-Hans': () => import('./locales/zh-Hans'),
  'zh-Hant': () => import('./locales/zh-Hant'),
}
const loaded = new Set<Locale>()
const pending = new Map<Locale, Promise<void>>()
let selection = 0

function loadLocale(value: Locale): Promise<void> {
  if (loaded.has(value))
    return Promise.resolve()
  const existing = pending.get(value)
  if (existing)
    return existing
  const loading = loaders[value]().then((messages) => {
    i18n.global.setLocaleMessage(value, messages.default)
    loaded.add(value)
  }).finally(() => pending.delete(value))
  pending.set(value, loading)
  return loading
}

export async function setLocale(value: Locale): Promise<boolean> {
  const current = ++selection
  try {
    await loadLocale('zh-Hans')
    await loadLocale(value)
  }
  catch (error) {
    if (current !== selection)
      return false
    throw error
  }
  if (current !== selection)
    return false
  locale.value = value
  applyLocale(value)
  return true
}
