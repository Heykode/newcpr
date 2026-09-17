/* eslint-disable test/no-import-node-test -- this regression uses Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

function load(relativePath) {
  const exports = {}
  const filename = new URL(relativePath, import.meta.url)
  const { outputText } = ts.transpileModule(readFileSync(filename, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, { exports }, { filename: filename.pathname })
  return exports
}

const { accountHasReloginTotp, reloginTotpEmailSet } = load('../src/views/accounts/relogin-availability.ts')

test('account 2FA indicator uses only valid relogin material and normalized email', () => {
  const emails = reloginTotpEmailSet([
    { email: ' Ready@Example.invalid ', hasTotp: true },
    { email: 'legacy@example.invalid', hasTotp: false },
  ])
  assert.equal(accountHasReloginTotp({ email: 'ready@example.invalid' }, emails), true)
  assert.equal(accountHasReloginTotp({ email: 'LEGACY@example.invalid' }, emails), false)
  assert.equal(accountHasReloginTotp({ email: null }, emails), false)
})

test('account identity renders a fixed top-left key badge without changing avatar size', () => {
  const component = readFileSync(new URL('../src/views/accounts/components/AccountIdentityCell.vue', import.meta.url), 'utf8')
  const page = readFileSync(new URL('../src/views/accounts/index.vue', import.meta.url), 'utf8')
  assert.match(component, /v-if="hasTotp"/)
  assert.match(component, /data-account-totp-mark/)
  assert.match(component, /absolute -left-1 -top-1/)
  assert.match(component, /size-4/)
  assert.match(component, /已配置 2FA，可用于失效重登/)
  assert.match(page, /:has-totp="hasReloginTotp\(row\)"/)
})
