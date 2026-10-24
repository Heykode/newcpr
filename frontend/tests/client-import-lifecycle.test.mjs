/* eslint-disable test/no-import-node-test -- Node regression runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

function harness(reveal = async () => 'synthetic-key') {
  const exports = {}
  const links = []
  const window = { location: { origin: 'https://fixture.invalid', href: '' } }
  const { outputText } = ts.transpileModule(readFileSync(new URL('../src/views/api-keys/composables/useApiKeyUse.ts', import.meta.url), 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  const dependencies = {
    vue,
    '@/api/constants': { API_BASE_URL: '' },
    '../utils/ccswitchImport': { buildCodexCcSwitchImportDeeplink: (input) => {
      links.push({ ...input })
      return 'ccswitch://synthetic'
    } },
  }
  runInNewContext(outputText, { exports, window, require: name => dependencies[name] })
  const scope = vue.effectScope()
  const state = scope.run(() => exports.useApiKeyUse({
    createdKey: vue.ref('synthetic-created'),
    createdKeyName: vue.ref('Synthetic'),
    revealPlaintextKey: reveal,
  }))
  return { state, scope, links, window }
}

test('client import waits for confirmation and close/disposal clears the plaintext', async () => {
  const app = harness()
  try {
    app.state.importCreatedKeyToCcs()
    assert.equal(app.state.showImportModal.value, true)
    assert.equal(app.links.length, 0)
    app.state.showImportModal.value = false
    app.state.clearImport()
    app.state.confirmImport()
    assert.equal(app.links.length, 0)
    await app.state.importToCcs({ id: 'key-a', name: 'Synthetic existing' })
    app.state.confirmImport()
    app.state.confirmImport()
    assert.deepEqual(app.links, [{ apiKey: 'synthetic-key', baseUrl: 'https://fixture.invalid/v1', providerName: 'Synthetic existing' }])
    app.state.importCreatedKeyToCcs()
    app.scope.stop()
    app.state.confirmImport()
    assert.equal(app.links.length, 1)
  }
  finally {
    app.scope.stop()
  }
})

test('late key reveal cannot reopen an unmounted page or replace a newer selection', async () => {
  const pending = []
  const app = harness(() => new Promise(resolve => pending.push(resolve)))
  const first = app.state.importToCcs({ id: 'a', name: 'First' })
  const second = app.state.importToCcs({ id: 'b', name: 'Second' })
  pending[1]('second-key')
  await second
  pending[0]('first-key')
  await first
  app.state.confirmImport()
  assert.equal(app.links[0].apiKey, 'second-key')
  app.state.showImportModal.value = false
  const late = app.state.importToCcs({ id: 'c', name: 'Late' })
  app.scope.stop()
  pending[2]('late-key')
  await late
  assert.equal(app.state.showImportModal.value, false)
  app.state.confirmImport()
  assert.equal(app.links.length, 1)
})
