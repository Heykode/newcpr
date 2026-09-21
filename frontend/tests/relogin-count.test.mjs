/* eslint-disable test/no-import-node-test -- this regression uses Node's built-in runner. */
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
function load(filename, dependencies = {}, source = readFileSync(filename, 'utf8')) {
  const { outputText } = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024, esModuleInterop: true },
  })
  const exports = {}
  runInNewContext(outputText, { exports, require: name => dependencies[name] ?? require(name) })
  return exports
}
const date = load(new URL('../src/utils/date.ts', import.meta.url))
const filename = new URL('../src/components/ReloginCountCell.vue', import.meta.url)
const { descriptor } = parse(readFileSync(filename, 'utf8'), { filename: filename.pathname })
const compiled = compileScript(descriptor, { id: 'relogin-count-test', inlineTemplate: true })
const cell = load(filename, { '@/utils/date': date }, compiled.content).default

for (const count of [0, 1, 2, 1000]) {
  test(`relogin count renders ${count} without treating success history as an error`, async () => {
    const html = await renderToString(createSSRApp(cell, {
      count,
      lastReloginAt: count ? '2026-09-16T02:03:04Z' : null,
    }))
    assert.equal(html.replace(/<[^>]*>/g, '').trim(), String(count))
    assert.match(html, /自启用统计起/)
    assert.match(html, count ? /2026-09-16 10:03:04 \(UTC\+8\)/ : /暂无成功重登记录/)
    assert.doesNotMatch(html, /text-cp-(error|warning|success)/)
  })
}

test('unresolved workspace is unknown rather than a misleading zero', async () => {
  const html = await renderToString(createSSRApp(cell, { count: null, lastReloginAt: null }))
  assert.equal(html.replace(/<[^>]*>/g, '').trim(), '—')
  assert.match(html, /未确定对应号池账号/)
})

test('account count column is sortable and follows last use and priority', () => {
  const columns = load(new URL('../src/views/accounts/constants.ts', import.meta.url), {
    '@/components/base/BaseTable/columns': { defineTableColumns: value => value },
  }).accountColumns
  const index = columns.findIndex(column => column.key === 'reloginCount')
  assert.equal(columns[index - 2].key, 'lastUsedAt')
  assert.equal(columns[index - 1].key, 'weight')
  assert.equal(columns[index + 1].key, 'addedAt')
  assert.equal(columns[index].sortable, true)
})
