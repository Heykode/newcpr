/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { Buffer } from 'node:buffer'
import { createHash } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'

const catalog = JSON.parse(readFileSync(new URL('../src/views/settings/components/outbound-user-agent-samples.json', import.meta.url), 'utf8'))

test('sample bytes and provenance stay pinned to the reviewed public release assets', () => {
  // Digests of exact selected rows, checked against the two asset bodies on 2026-09-24.
  // Full asset hashes below also matched GitHub release metadata. No official verification implied.
  const pinned = {
    desktop: {
      release: '26.917.62051',
      assetId: 584062339,
      sha256: '1977adfbdaedb8cd6111608a572c94cd4cde205ad7faeec60c6803dabc081ccb',
      rows: '508cdd33a77a0941bfaceb4755a3327f790de587363429dccd4e4c9138a97d94',
      count: 3,
    },
    cli: {
      release: '0.156.1',
      assetId: 584069027,
      sha256: 'f695d8474bd0e4856bb6f225a73d0a97949d24d7089be5e67b2d0c4362568c70',
      rows: '03372e0df8360538d6ac3187e86405c24c3fce0a4c8c098d6488a31d46d94b47',
      count: 10,
    },
  }
  for (const [key, expected] of Object.entries(pinned)) {
    const source = catalog.sources[key]
    assert.equal(source.release, expected.release)
    assert.equal(source.assetId, expected.assetId)
    assert.equal(source.sha256, expected.sha256)
    assert.match(source.commit, /^[0-9a-f]{40}$/)
    assert.equal(source.url, `https://github.com/${source.repository}/releases/download/v${expected.release}/ua-matrix.json`)
    const samples = catalog.samples.filter(sample => (sample.client === 'Desktop' ? 'desktop' : 'cli') === key)
    assert.equal(samples.length, expected.count)
    assert.equal(createHash('sha256').update(JSON.stringify(samples.map(sample => sample.userAgent))).digest('hex'), expected.rows)
  }
})

test('samples only offer supported exact platform rows without inventing or normalizing bytes', () => {
  assert.equal(new Set(catalog.samples.map(sample => sample.id)).size, catalog.samples.length)
  const cliPlatforms = ['macos-arm64', 'macos-x64', 'windows-x64', 'linux-ubuntu-x64', 'linux-ubuntu-arm64']
  for (const client of ['Desktop', 'CLI', 'Exec']) {
    const samples = catalog.samples.filter(sample => sample.client === client)
    assert.deepEqual(samples.map(sample => sample.environment), client === 'Desktop' ? cliPlatforms.slice(0, 3) : cliPlatforms)
    for (const sample of samples) {
      assert(Buffer.byteLength(sample.userAgent) <= 512)
      assert.match(sample.userAgent, /^[\x20-\x7E]+$/)
      assert.equal(sample.userAgent.trim(), sample.userAgent)
      const originator = { Desktop: 'Codex Desktop', CLI: 'codex-tui', Exec: 'codex_exec' }[client]
      const release = catalog.sources[client === 'Desktop' ? 'desktop' : 'cli'].release
      assert(sample.userAgent.startsWith(`${originator}/`))
      assert(sample.userAgent.endsWith(` (${originator}; ${release})`))
      if (client !== 'Desktop')
        assert(sample.userAgent.startsWith(`${originator}/${release} (`))
    }
  }
})
