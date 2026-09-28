/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

const exports = {}
const { outputText } = ts.transpileModule(readFileSync(new URL('../src/views/proxies/mihomoNodes.ts', import.meta.url), 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
})
runInNewContext(outputText, { exports })
const { batchTargets, checkOutcome, filterNodes, runNodeBatch } = exports
const nodes = [
  { name: 'a', displayName: 'Tokyo A', subscriptionIds: ['first'], dynamic: false, state: 'ready', countryCode: 'JP', countryBlocked: false },
  { name: 'b', displayName: 'Singapore B', subscriptionIds: ['first', 'second'], dynamic: false, state: 'ready', countryCode: 'SG', countryBlocked: true },
  { name: 'c', displayName: 'Tokyo C', subscriptionIds: ['second'], dynamic: false, state: 'disabled', countryCode: null, countryBlocked: true },
  { name: 'd', displayName: 'Dynamic D', subscriptionIds: [], dynamic: true, state: 'ready', countryCode: null, countryBlocked: false },
]
const defaults = { source: 'all', state: 'all', search: '', dynamicOnly: false }
const filteredNames = filters => Array.from(filterNodes(nodes, { ...defaults, ...filters }), node => node.name)

test('source filtering supports shared subscriptions and isolates dynamic proxies', () => {
  assert.deepEqual(filteredNames({ source: 'source:first' }), ['a', 'b'])
  assert.deepEqual(filteredNames({ source: 'source:second' }), ['b', 'c'])
  assert.deepEqual(filteredNames({ source: 'source:missing' }), [])
  assert.deepEqual(filteredNames({ source: 'subscription' }), ['a', 'b', 'c'])
  assert.deepEqual(filteredNames({ source: 'dynamic' }), ['d'])
  assert.deepEqual(filteredNames({ dynamicOnly: true }), ['d'])
})

test('source, state, country and case insensitive name filters intersect', () => {
  assert.deepEqual(filteredNames({ state: 'blocked' }), ['b', 'c'])
  assert.deepEqual(filteredNames({ state: 'disabled' }), ['c'])
  assert.deepEqual(filteredNames({ source: 'source:first', search: ' sg ' }), ['b'])
  assert.deepEqual(filteredNames({ source: 'source:first', search: 'TOKYO' }), ['a'])
})

test('selected targets are intersected without silently expanding disappeared selections', () => {
  assert.deepEqual(Array.from(batchTargets(nodes, new Set(['a', 'c'])), node => node.name), ['a', 'c'])
  assert.equal(batchTargets(nodes, new Set()).length, 4)
  assert.equal(batchTargets(nodes.slice(0, 1), new Set(['b'])).length, 0)
})

test('quality summaries distinguish reachability, challenge, warning and failure', () => {
  const result = statuses => ({ success: true, quality: { checks: statuses.map(status => ({ status, httpStatus: 401 })) } })
  assert.equal(checkOutcome(result(['pass']), true), 'passed')
  assert.equal(checkOutcome(result(['pass', 'warn']), true), 'warning')
  assert.equal(checkOutcome(result(['fail', 'challenge']), true), 'challenge')
  assert.equal(checkOutcome(result(['fail']), true), 'failed')
  assert.equal(checkOutcome(result([]), true), 'failed')
  assert.equal(checkOutcome({ success: false, quality: result(['pass']).quality }, true), 'failed')
  assert.equal(checkOutcome({ success: true, quality: null }, true), 'failed')
  assert.equal(checkOutcome({ success: true }, false), 'passed')
  assert.equal(checkOutcome({ success: false }, false), 'failed')
})

for (const concurrency of [2, 3]) {
  test(`batch workers run concurrently at exactly ${concurrency}, continue after failure and settle once`, async () => {
    let running = 0
    let peak = 0
    const invoked = []
    const outcomes = []
    const all = Array.from({ length: 9 }, (_, id) => ({ name: String(id) }))
    await runNodeBatch(all, concurrency, async (node) => {
      running++
      peak = Math.max(peak, running)
      invoked.push(node.name)
      await new Promise(resolve => setTimeout(resolve, 5))
      running--
      if (node.name === '1')
        throw new Error('synthetic failure')
      return node.name === '3' ? 'skipped' : 'passed'
    }, outcome => outcomes.push(outcome), () => false)
    assert.equal(peak, concurrency)
    assert.equal(running, 0)
    assert.equal(new Set(invoked).size, all.length)
    assert.equal(outcomes.length, all.length)
    assert.equal(outcomes.filter(value => value === 'failed').length, 1)
    assert.equal(outcomes.filter(value => value === 'skipped').length, 1)
  })
}

test('unmount stops queued dispatch and suppresses late progress', async () => {
  let disposed = false
  const started = []
  const outcomes = []
  const releases = []
  const promise = runNodeBatch(nodes, 2, async (node) => {
    started.push(node.name)
    await new Promise(resolve => releases.push(resolve))
    return 'passed'
  }, outcome => outcomes.push(outcome), () => disposed)
  assert.deepEqual(started, ['a', 'b'])
  disposed = true
  releases.forEach(release => release())
  await promise
  assert.deepEqual(started, ['a', 'b'])
  assert.deepEqual(outcomes, [])
})

test('empty and already disposed batches dispatch nothing', async () => {
  let count = 0
  const run = async () => {
    count++
    return 'passed'
  }
  await runNodeBatch([], 3, run, () => {}, () => false)
  await runNodeBatch(nodes, 3, run, () => {}, () => true)
  assert.equal(count, 0)
})
