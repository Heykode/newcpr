/* eslint-disable test/no-import-node-test -- Node regression runner. */
import assert from 'node:assert/strict'
import { readdirSync, readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vueI18n from 'vue-i18n'

const source = new URL('../src/', import.meta.url)
const languages = ['zh-Hans', 'zh-Hant', 'en']
function messages(language) {
  const directory = new URL(`i18n/locales/${language}/`, source)
  return Object.assign({}, ...readdirSync(directory).filter(name => name.endsWith('.json')).map(name => JSON.parse(readFileSync(new URL(name, directory), 'utf8'))))
}

function* sources(directory) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = new URL(entry.name, directory)
    if (entry.isDirectory())
      yield* sources(new URL(`${entry.name}/`, directory))
    else if (/\.(?:vue|ts)$/.test(entry.name))
      yield path
  }
}

test('every literal translation key used by the application exists in all three locales', () => {
  const catalogs = languages.map(language => [language, messages(language)])
  const missing = []
  for (const path of sources(source)) {
    const content = readFileSync(path, 'utf8')
    for (const [, key] of content.matchAll(/\bt\(\s*['"]([^'"]+)['"]/g)) {
      for (const [language, catalog] of catalogs) {
        const value = key.split('.').reduce((node, field) => node?.[field], catalog)
        if (typeof value !== 'string')
          missing.push(`${language}: ${key} (${path.pathname})`)
      }
    }
  }
  assert.deepEqual(missing, [])
})

function deferred() {
  let resolve
  const promise = new Promise(done => resolve = done)
  return { resolve, promise }
}

test('lazy locale loading publishes only the latest selection and keeps the fallback loaded', async () => {
  const gates = Object.fromEntries(languages.map(language => [language, deferred()]))
  const applied = []
  const exports = {}
  const { outputText } = ts.transpileModule(readFileSync(new URL('i18n/index.ts', source), 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, {
    exports,
    require(name) {
      if (name === 'vue-i18n')
        return vueI18n
      if (name === './locale')
        return { readInitialLocale: () => 'zh-Hans', applyLocale: value => applied.push(value) }
      const language = name.replace('./locales/', '')
      assert.ok(language in gates, name)
      return gates[language].promise
    },
  })
  const first = exports.setLocale('en')
  gates['zh-Hans'].resolve({ default: messages('zh-Hans') })
  await Promise.resolve()
  const latest = exports.setLocale('zh-Hant')
  gates['zh-Hant'].resolve({ default: messages('zh-Hant') })
  assert.equal(await latest, true)
  gates.en.resolve({ default: messages('en') })
  assert.equal(await first, false)
  assert.equal(exports.locale.value, 'zh-Hant')
  assert.deepEqual(applied, ['zh-Hant'])
  assert.ok(exports.i18n.global.availableLocales.includes('zh-Hans'))
})
