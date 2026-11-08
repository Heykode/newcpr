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

test('theme overrides allow alpha only for supported controls and retain native namespaces', () => {
  const { normalizeThemeCustomization, supportsThemeTokenAlpha } = load(resolve(root, 'core/normalize.ts'))
  const { createThemeVars } = load(resolve(root, 'namespace.ts'))
  const overrides = normalizeThemeCustomization({ tokenOverrides: {
    '--cp-input-bg': '#11223344',
    '--cp-button-secondary-bg': '#22334455',
    '--cp-card-bg': '#11223344',
    '--cp-color-text': '#112233',
  } }).tokenOverrides
  assert.equal(overrides['--cp-input-bg'], '#11223344')
  assert.equal(overrides['--cp-button-secondary-bg'], '#22334455')
  assert.equal(overrides['--cp-card-bg'], undefined)
  assert.equal(supportsThemeTokenAlpha('--cp-input-bg'), true)
  assert.equal(createThemeVars({ 'input-bg': '#11223344' })['--cp-input-bg'], '#11223344')
})

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

test('upstream material and complete profiles keep explicit customization last', () => {
  const { createTheme } = load(resolve(root, 'core/resolve.ts'))
  const { themeProfiles } = load(resolve(root, 'core/constants.ts'))
  for (const name of ['light', 'dark']) {
    for (const colorId of Object.keys(themeProfiles)) {
      const profile = themeProfiles[colorId]
      const full = createTheme({ name, colorId, ...profile })
      assert.equal(full.preset, profile.preset)
      assert.ok(full.tokens['--cp-card-bg-image'])
      assert.ok(full.tokens['--cp-layout-sider-bg-image'])
    }
    const soft = createTheme({ name, preset: 'soft', customization: { seed: { borderRadius: 7 }, component: { cardBorderRadius: 11 } } })
    assert.equal(soft.seedTokens.borderRadius, 7)
    assert.equal(soft.tokens['--cp-card-border-radius'], '11px')
    assert.equal(createTheme({ name }).preset, 'default')
    assert.notEqual(createTheme({ name, preset: 'soft' }).tokens['--cp-card-bg-image'], createTheme({ name }).tokens['--cp-card-bg-image'])
  }
})
