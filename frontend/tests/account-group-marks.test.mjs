/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import { createSSRApp, defineComponent, h } from 'vue'
import { compileScript, parse } from 'vue/compiler-sfc'
import { renderToString } from 'vue/server-renderer'

const require = createRequire(import.meta.url)
const layoutExports = {}
const layoutFile = new URL('../src/components/account-group-layout.ts', import.meta.url)
runInNewContext(ts.transpileModule(readFileSync(layoutFile, 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
}).outputText, { exports: layoutExports })
const { accountGroupLayout } = layoutExports
const filename = new URL('../src/components/AccountGroupMarks.vue', import.meta.url)
const { descriptor } = parse(readFileSync(filename, 'utf8'), { filename: filename.pathname })
const compiled = compileScript(descriptor, { id: 'account-groups-test', inlineTemplate: true })
const { outputText } = ts.transpileModule(compiled.content, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
})
const exports = {}
const popover = defineComponent({
  props: ['hoverDelay', 'placement', 'trigger'],
  setup: (props, { slots }) => () => h('div', { 'data-hover-delay': props.hoverDelay }, [
    slots.trigger?.({ open: true }),
    slots.default?.({ open: true }),
  ]),
})
runInNewContext(outputText, {
  exports,
  require: (name) => {
    if (name === './account-group-layout')
      return layoutExports
    if (name === '@/components/base/BasePopover.vue')
      return popover
    if (name === '@/composables/useThemeColor')
      return { useThemeColor: () => (_variable, fallback) => fallback }
    return require(name)
  },
}, { filename: filename.pathname })

function group(index) {
  return {
    id: `group-${index}`,
    name: `Group ${index} ${'long-name-'.repeat(12)}`,
    color: '#1677ff',
    enabled: index !== 2,
  }
}

test('one or two long group names still have a keyboard-accessible full-name popover', async () => {
  for (const groups of [[group(1)], [group(1), group(2)]]) {
    const html = await renderToString(createSSRApp(exports.default, { groups }))
    assert.match(html, /data-hover-delay="100"/)
    assert.match(html, /<button[^>]*type="button"[^>]*aria-expanded="true"/)
    const items = [...html.matchAll(/<span\s[^>]*role="listitem"[^>]*>(.*?)<\/span><\/span>/gs)]
    assert.equal(items.length, groups.length)
    for (const [index, item] of items.entries()) {
      assert.ok(item[1].includes(groups[index].name))
      assert.ok(!item[0].includes('truncate'))
      assert.ok(item[0].includes('break-all'))
    }
    assert.ok(!html.includes('+0'))
  }
})

test('group summary stays bounded while the popover retains every group and disabled status', async () => {
  const groups = Array.from({ length: 5 }, (_, index) => group(index))
  const html = await renderToString(createSSRApp(exports.default, { groups }))
  const trigger = html.match(/<button\b[^>]*>(.*?)<\/button>/s)?.[1]
  assert.ok(trigger)
  assert.ok(trigger.includes(groups[0].name))
  assert.ok(trigger.includes(groups[1].name))
  assert.ok(!trigger.includes(groups[2].name))
  assert.ok(trigger.includes('+3'))
  assert.equal([...html.matchAll(/role="listitem"/g)].length, 5)
  assert.match(html, /已禁用/)
})

test('empty account groups do not create a popover or an empty button', async () => {
  const html = await renderToString(createSSRApp(exports.default, { groups: [] }))
  assert.ok(!html.includes('<button'))
  assert.ok(!html.includes('role="list"'))
})

test('account table centers wrapping summaries and measures labels without changing inline callers', async () => {
  const groups = Array.from({ length: 5 }, (_, index) => group(index))
  const stacked = await renderToString(createSSRApp(exports.default, { groups, layout: 'stacked' }))
  const inline = await renderToString(createSSRApp(exports.default, { groups }))
  const trigger = stacked.match(/<button\b[^>]*>(.*?)<\/button>/s)?.[0]
  assert.ok(trigger.includes('flex-wrap justify-center'))
  assert.equal([...trigger.matchAll(/text-\[13px\]/g)].length, 3)
  assert.ok(!trigger.includes('col-start-1'))
  assert.ok(stacked.includes('aria-hidden="true"'))
  assert.equal([...stacked.matchAll(/data-group-measure/g)].length, 5)
  assert.equal([...stacked.matchAll(/role="listitem"/g)].length, 5)
  assert.ok(inline.includes('text-[10px]'))
  assert.ok(!inline.includes('data-group-measure'))
})

test('short names use available space beyond the old two-group limit', () => {
  for (const count of [1, 2, 3, 4, 5, 6]) {
    assert.equal(accountGroupLayout(Array.from({ length: count }).fill(42), 160, 26).count, count)
  }
  assert.equal(accountGroupLayout(Array.from({ length: 7 }).fill(42), 160, 26).count, 5)
  assert.equal(accountGroupLayout(Array.from({ length: 7 }).fill(42), 260, 26).count, 7)
})

test('two-row group packing reserves overflow space and bounds long names', () => {
  const long = accountGroupLayout(Array.from({ length: 5 }).fill(400), 160, 26)
  assert.equal(long.count, 2)
  assert.equal(long.maxWidth, 128)
  assert.equal(accountGroupLayout([42, 90, 42, 42, 90], 160, 26).count, 4)
  assert.equal(accountGroupLayout([400, 400], 160, 26).count, 2)
  assert.equal(accountGroupLayout([], 160, 26).count, 0)
  assert.equal(accountGroupLayout([42, 42, 42], 0, 26).count, 2)
})
