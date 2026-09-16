/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

function load(relativePath, dependencies = {}) {
  const exports = {}
  const filename = new URL(relativePath, import.meta.url)
  const { outputText } = ts.transpileModule(readFileSync(filename, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, { exports, require: name => dependencies[name] }, { filename: filename.pathname })
  return exports
}

const table = load('../src/components/base/BaseTable/columns.ts')
const { accountColumns } = load('../src/views/accounts/constants.ts', {
  '@/components/base/BaseTable/columns': table,
})

test('account status and plan columns share centered header and cell alignment', () => {
  const columns = table.resolveColumns(accountColumns)
  for (const key of ['status', 'planType']) {
    const column = columns.find(column => column.key === key)
    assert.equal(column.align, 'center')
    assert.equal(table.alignClass(column), 'text-center')
    assert.equal(column.sortable, true)
  }
})

test('account table keeps group and last-used columns compact and grows only identity and usage', () => {
  const columns = table.resolveColumns(accountColumns)
  const find = key => columns.find(column => column.key === key)
  assert.equal(find('groups').basisWidth, 184)
  assert.equal(find('lastUsedAt').basisWidth, 112)
  for (const key of ['groups', 'lastUsedAt', 'selection', 'actions']) {
    const column = find(key)
    assert.equal(table.columnStyle(column, columns, 'content').width, `${column.basisWidth}px`)
  }
  for (const key of ['identity', 'usage'])
    assert.match(table.columnStyle(find(key), columns, 'content').width, /\* 0\.5\)$/)
})

test('content layout recomputes growth when columns are hidden and retains a proportional fallback', () => {
  const columns = table.resolveColumns(accountColumns.filter(column => column.key !== 'usage'))
  const identity = columns.find(column => column.key === 'identity')
  const style = table.columnStyle(identity, columns, 'content')
  assert.match(style.width, /\* 1\)$/)
  assert.ok(style.width.includes(`${table.minimumTableWidth(columns)}px`))
  const noGrowColumns = columns.filter(column => column.key !== 'identity')
  for (const column of noGrowColumns)
    assert.match(table.columnStyle(column, noGrowColumns, 'content').width, /^[\d.]+%$/)
})

test('existing tables keep proportional sizing unless they explicitly opt in', () => {
  const columns = table.resolveColumns(accountColumns)
  for (const column of columns) {
    const style = table.columnStyle(column, columns)
    assert.equal(style.width, `${column.basisWidth / table.minimumTableWidth(columns) * 100}%`)
    assert.equal(style.minWidth, `${column.basisWidth}px`)
  }
})

test('invalid growth and sticky column growth cannot produce invalid widths or offsets', () => {
  const columns = table.resolveColumns([
    { key: 'selection', kind: 'selection', grow: 20 },
    { key: 'invalid', grow: Number.NaN },
    { key: 'negative', grow: -5 },
    { key: 'infinite', grow: Number.POSITIVE_INFINITY },
    { key: 'identity', kind: 'identity', grow: 1 },
  ])
  assert.equal(columns[0].stickyOffset, 0)
  for (const column of columns.slice(0, -1))
    assert.equal(table.columnStyle(column, columns, 'content').width, `${column.basisWidth}px`)
  assert.match(table.columnStyle(columns.at(-1), columns, 'content').width, /\* 1\)$/)
})
