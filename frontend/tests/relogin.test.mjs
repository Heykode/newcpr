/* eslint-disable test/no-import-node-test -- this suite uses Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

function load(path, dependencies = {}) {
  const exports = {}
  const { outputText } = ts.transpileModule(readFileSync(new URL(path, import.meta.url), 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, { exports, TextEncoder, require: name => dependencies[name] })
  return exports
}
const { importPreview } = load('../src/views/relogin/import-preview.ts')

test('relogin preview retains source line numbers, normalizes email and never returns secrets', () => {
  const rows = importPreview('\uFEFFTest@Example.invalid----p%----ss----jbsw y3dp-ehpk3pxp\n\nsecond@example.invalid----p----JBSWY3DPEHPK3PXP')
  assert.equal(rows.length, 2)
  assert.equal(rows[0].email, 'test@example.invalid')
  assert.equal(rows[0].valid, true)
  assert.equal(rows[1].line, 3)
  assert.equal(JSON.stringify(rows).includes('JBSWY'), false)
  assert.equal(JSON.stringify(rows).includes('p%'), false)
})

test('relogin preview rejects malformed secrets, empty passwords and case-folded duplicates', () => {
  const invalid = [
    'bad----p----JBSWY3DPEHPK3PXP',
    'test@example.invalid--------JBSWY3DPEHPK3PXP',
    'test@example.invalid----p----JBSWY3DPEHPK3PXP0',
    'test@example.invalid----p----JBSWY3DPEHPK3P',
  ]
  for (const text of invalid)
    assert.equal(importPreview(text)[0].valid, false)
  const repeated = importPreview('test@example.invalid----p----JBSWY3DPEHPK3PXP\nTEST@example.invalid----p----JBSWY3DPEHPK3PXP')
  assert.equal(repeated[1].valid, false)
})

test('relogin preview rejects mailbox formats but accepts Outlook accounts with TOTP', () => {
  for (const text of [
    'person@outlook.com----test-only-password----123e4567-e89b-12d3-a456-426614174000',
    'person@outlook.com----test-only-password----123e4567-e89b-12d3-a456-426614174000----!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!',
    'person@outlook.com----test-only-password----123e4567-e89b-12d3-a456-426614174000----JBSWY3DPEHPK3PXP',
    'person@outlook.com|synthetic-mailbox-token|123e4567-e89b-12d3-a456-426614174000',
  ]) {
    const rows = importPreview(text)
    assert.equal(rows[0].valid, false)
    assert.equal(JSON.stringify(rows).includes('test-only-password'), false)
    assert.equal(JSON.stringify(rows).includes('synthetic-mailbox-token'), false)
  }
  const rows = importPreview('person@outlook.com----p----JBSWY3DPEHPK3PXP')
  assert.equal(rows[0].valid, true)
  assert.equal(rows[0].email, 'person@outlook.com')
})

test('relogin push sends exactly the confirmed row versions and does not replay', async () => {
  const calls = []
  const { pushRelogin } = load('../src/api/modules/relogin.ts', {
    '../request': async (config) => {
      calls.push(config)
      throw new Error('version changed')
    },
  })
  await assert.rejects(pushRelogin([{ id: 'a', revision: 7 }, { id: 'b', revision: 12 }]), /version changed/)
  assert.equal(calls.length, 1)
  assert.deepEqual(JSON.parse(JSON.stringify(calls[0].data)), {
    ids: ['a', 'b'],
    revisions: { a: 7, b: 12 },
  })
})
