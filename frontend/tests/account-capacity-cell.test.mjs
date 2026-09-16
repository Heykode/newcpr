/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import { createSSRApp } from 'vue'
import { compileScript, parse } from 'vue/compiler-sfc'
import { renderToString } from 'vue/server-renderer'

const require = createRequire(import.meta.url)
const filename = new URL('../src/views/accounts/components/AccountCapacityCell.vue', import.meta.url)
const { descriptor } = parse(readFileSync(filename, 'utf8'), { filename: filename.pathname })
const compiled = compileScript(descriptor, { id: 'account-capacity-test', inlineTemplate: true })
const { outputText } = ts.transpileModule(compiled.content, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
})
const exports = {}
runInNewContext(outputText, { exports, require }, { filename: filename.pathname })

const unknown = '\u2014'
const maxU32 = 4_294_967_295
const stateClasses = [
  'text-cp-text-secondary',
  'text-cp-success-text',
  'text-cp-warning-text',
  'text-cp-error-text',
]
const nonNeutralClass = /(?:text|bg)-cp-(?:success|warning|error|info|primary|blue)|shadow|font-(?:emphasis|heavy|bold|semibold)/

function classesFromHtml(html) {
  return html.match(/\bclass="([^"]*)"/)?.[1].split(/\s+/) ?? []
}

function capacityPart(html, part) {
  const matches = [...html.matchAll(new RegExp(`<span\\b[^>]*\\bdata-capacity-${part}(?:="")?(?=[\\s>])[^>]*>([^<]*)</span>`, 'g'))]
  assert.equal(matches.length, 1, `one ${part} element: ${html}`)
  return {
    html: matches[0][0],
    text: matches[0][1].trim(),
    classes: classesFromHtml(matches[0][0]),
  }
}

async function renderCapacity(props) {
  const html = await renderToString(createSSRApp(exports.default, props))
  const root = html.match(/^<div\b[^>]*>/)?.[0]
  assert.ok(root, html)
  assert.match(root, /\bdata-account-capacity(?:="")?(?=[\s>])/)
  assert.match(root, /role="group"/)
  assert.match(root, /aria-label="账号并发容量"/)
  assert.doesNotMatch(classesFromHtml(root).join(' '), nonNeutralClass)
  const used = capacityPart(html, 'used')
  const separator = capacityPart(html, 'separator')
  const limit = capacityPart(html, 'limit')
  assert.equal(separator.text, '/')
  assert.match(separator.html, /aria-hidden="true"/)
  assert.ok(separator.classes.includes('text-cp-text-secondary'), separator.html)
  assert.ok(limit.classes.includes('text-cp-text-secondary'), limit.html)
  for (const part of [separator, limit]) {
    assert.ok(part.classes.includes('font-normal'), part.html)
    assert.doesNotMatch(part.classes.join(' '), nonNeutralClass)
  }
  assert.ok(used.classes.includes('font-emphasis'), used.html)
  for (const [part, label] of [[used, '实时并发'], [limit, '并发上限']]) {
    assert.match(part.html, /role="img"/)
    const value = part.text === unknown ? '未知' : part.text
    assert.ok(part.html.includes(`aria-label="${label} ${value}"`), part.html)
  }
  return { html, root, used, separator, limit }
}

for (const [name, props, used, limit, color] of [
  ['omitted values stay unknown', {}, unknown, unknown, 'text-cp-text-secondary'],
  ['nullable values stay unknown', { inFlight: null, effectiveConcurrencyLimit: null, concurrencyLimit: null }, unknown, unknown, 'text-cp-text-secondary'],
  ['undefined usage with a known limit', { inFlight: undefined, effectiveConcurrencyLimit: 5 }, unknown, '5', 'text-cp-text-secondary'],
  ['null usage with an override', { inFlight: null, concurrencyLimit: 5 }, unknown, '5', 'text-cp-text-secondary'],
  ['zero usage is idle', { inFlight: 0, effectiveConcurrencyLimit: 5 }, '0', '5', 'text-cp-text-secondary'],
  ['negative usage is neutral', { inFlight: -1, effectiveConcurrencyLimit: 5 }, '-1', '5', 'text-cp-text-secondary'],
  ['typical active usage', { inFlight: 2, effectiveConcurrencyLimit: 5 }, '2', '5', 'text-cp-success-text'],
  ['unknown omitted limit with active usage', { inFlight: 2 }, '2', unknown, 'text-cp-success-text'],
  ['unknown nullable limit with active usage', { inFlight: 2, effectiveConcurrencyLimit: null, concurrencyLimit: null }, '2', unknown, 'text-cp-success-text'],
  ['unknown limit with idle usage', { inFlight: 0 }, '0', unknown, 'text-cp-text-secondary'],
  ['larger effective limit wins for display and color', { inFlight: 4, effectiveConcurrencyLimit: 10, concurrencyLimit: 2 }, '4', '10', 'text-cp-success-text'],
  ['smaller effective limit wins for display and color', { inFlight: 4, effectiveConcurrencyLimit: 4, concurrencyLimit: 10 }, '4', '4', 'text-cp-error-text'],
  ['null effective limit falls back to override', { inFlight: 3, effectiveConcurrencyLimit: null, concurrencyLimit: 4 }, '3', '4', 'text-cp-warning-text'],
  ['undefined effective limit falls back to override', { inFlight: 1, effectiveConcurrencyLimit: undefined, concurrencyLimit: 2 }, '1', '2', 'text-cp-success-text'],
  ['null override preserves effective limit', { inFlight: 4, effectiveConcurrencyLimit: 5, concurrencyLimit: null }, '4', '5', 'text-cp-warning-text'],
  ['zero effective limit is not replaced by a positive override', { inFlight: 1, effectiveConcurrencyLimit: 0, concurrencyLimit: 5 }, '1', '0', 'text-cp-error-text'],
  ['zero usage with zero effective limit stays idle', { inFlight: 0, effectiveConcurrencyLimit: 0, concurrencyLimit: 5 }, '0', '0', 'text-cp-text-secondary'],
  ['unknown usage with zero limit stays neutral', { inFlight: null, effectiveConcurrencyLimit: 0 }, unknown, '0', 'text-cp-text-secondary'],
  ['zero override is preserved on nullable fallback', { inFlight: 1, effectiveConcurrencyLimit: null, concurrencyLimit: 0 }, '1', '0', 'text-cp-error-text'],
  ['zero usage with zero override stays idle', { inFlight: 0, concurrencyLimit: 0 }, '0', '0', 'text-cp-text-secondary'],
  ['just below 75 percent', { inFlight: 7499, effectiveConcurrencyLimit: 10000 }, '7499', '10000', 'text-cp-success-text'],
  ['exactly 75 percent', { inFlight: 7500, effectiveConcurrencyLimit: 10000 }, '7500', '10000', 'text-cp-warning-text'],
  ['just above 75 percent', { inFlight: 7501, effectiveConcurrencyLimit: 10000 }, '7501', '10000', 'text-cp-warning-text'],
  ['just below capacity', { inFlight: 9999, effectiveConcurrencyLimit: 10000 }, '9999', '10000', 'text-cp-warning-text'],
  ['exactly at capacity', { inFlight: 10000, effectiveConcurrencyLimit: 10000 }, '10000', '10000', 'text-cp-error-text'],
  ['over capacity', { inFlight: 10001, effectiveConcurrencyLimit: 10000 }, '10001', '10000', 'text-cp-error-text'],
  ['maximum u32 limit is rendered without truncation', { inFlight: 2, effectiveConcurrencyLimit: maxU32 }, '2', String(maxU32), 'text-cp-success-text'],
  ['maximum u32 usage and limit are rendered without truncation', { inFlight: maxU32, effectiveConcurrencyLimit: maxU32 }, String(maxU32), String(maxU32), 'text-cp-error-text'],
  ['maximum u32 threshold lower neighbor', { inFlight: 3_221_225_471, effectiveConcurrencyLimit: maxU32 }, '3221225471', String(maxU32), 'text-cp-success-text'],
  ['maximum u32 threshold upper neighbor', { inFlight: 3_221_225_472, effectiveConcurrencyLimit: maxU32 }, '3221225472', String(maxU32), 'text-cp-warning-text'],
]) {
  test(`capacity: ${name}`, async () => {
    const rendered = await renderCapacity(props)
    assert.equal(rendered.used.text, used)
    assert.equal(rendered.limit.text, limit)
    assert.deepEqual(rendered.used.classes.filter(value => stateClasses.includes(value)), [color])
    assert.equal(rendered.html.replace(/<[^>]*>/g, '').replace(/\s+/g, ''), `${used}/${limit}`)
  })
}

test('capacity layout keeps typical values inline and allows long values to wrap inside the cell', async () => {
  for (const inFlight of [2, maxU32]) {
    const rendered = await renderCapacity({ inFlight, effectiveConcurrencyLimit: inFlight === 2 ? 5 : maxU32 })
    const rootClasses = classesFromHtml(rendered.root)
    for (const value of ['inline-flex', 'flex-wrap', 'min-w-0', 'max-w-full', 'whitespace-normal', 'text-cp-sm', 'font-normal'])
      assert.ok(rootClasses.includes(value), rendered.root)
    for (const part of [rendered.used, rendered.limit]) {
      for (const value of ['min-w-0', 'max-w-full', 'wrap-anywhere'])
        assert.ok(part.classes.includes(value), part.html)
    }
    assert.doesNotMatch(rendered.html, /\b(?:truncate|whitespace-nowrap|break-keep|text-ellipsis|min-w-20)\b/)
  }
})
