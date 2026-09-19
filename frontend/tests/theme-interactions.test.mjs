/* eslint-disable test/no-import-node-test -- local theme regression runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, resolve } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

const require = createRequire(import.meta.url)
const cache = new Map()
function load(filename) {
  if (cache.has(filename))
    return cache.get(filename)
  const exports = {}
  cache.set(filename, exports)
  const { outputText } = ts.transpileModule(readFileSync(filename, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, {
    exports,
    require: name => name.startsWith('.') ? load(resolve(dirname(filename), `${name}.ts`)) : require(name),
  })
  return exports
}
const root = fileURLToPath(new URL('../src/theme/', import.meta.url))
const { resolveTheme } = load(resolve(root, 'core/resolve.ts'))
const { THEME_COLOR_PRESETS } = load(resolve(root, 'core/constants.ts'))

test('all theme presets keep equal hover/focus feedback even without decorative shadows', () => {
  for (const theme of ['light', 'dark']) {
    for (const preset of THEME_COLOR_PRESETS) {
      const { tokens } = resolveTheme(theme, preset.id, '#1677ff', { seed: { shadowStrength: 0 } })
      assert.equal(tokens['--cp-input-hover-bg'], tokens['--cp-input-active-bg'])
      assert.equal(tokens['--cp-input-hover-shadow'], tokens['--cp-input-active-shadow'])
      assert.match(tokens['--cp-input-active-shadow'], /0 0 0 3px/)
      assert.notEqual(tokens['--cp-input-error-active-bg'], tokens['--cp-input-active-bg'])
    }
  }
})
