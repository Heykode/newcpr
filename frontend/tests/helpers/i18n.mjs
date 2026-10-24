import { readdirSync, readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { createI18n } from 'vue-i18n'

const directory = new URL('../../src/i18n/locales/zh-Hans/', import.meta.url)
const messages = Object.assign({}, ...readdirSync(directory)
  .filter(name => name.endsWith('.json'))
  .map(name => JSON.parse(readFileSync(new URL(name, directory), 'utf8'))))

// Use the actual messages and interpolation engine in VM component tests.
export const i18n = createI18n({
  legacy: false,
  locale: 'zh-Hans',
  fallbackLocale: false,
  messages: { 'zh-Hans': messages },
})
export const { t, locale } = i18n.global

export function createTestRequire(url) {
  const require = createRequire(url)
  return Object.assign(name => name === '@/i18n' ? { i18n, t, locale } : require(name), require)
}
