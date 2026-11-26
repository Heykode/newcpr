import { readdirSync, readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
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
  let numberExports
  const resolve = (name) => {
    if (name === '@/i18n')
      return { i18n, t, locale }
    if (name === '@/utils/number') {
      if (!numberExports) {
        numberExports = {}
        const source = readFileSync(new URL('../../src/utils/number.ts', import.meta.url), 'utf8')
        const { outputText } = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 } })
        runInNewContext(outputText, { exports: numberExports, require: resolve })
      }
      return numberExports
    }
    return require(name)
  }
  return Object.assign(resolve, require)
}
