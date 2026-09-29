/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'
import { compileScript, parse } from 'vue/compiler-sfc'

const filename = new URL('../src/views/dashboard/components/WireProfileCard.vue', import.meta.url)
const { descriptor } = parse(readFileSync(filename, 'utf8'), { filename: filename.pathname })
const compiled = compileScript(descriptor, { id: 'wire-profile-card-test' })
const { outputText } = ts.transpileModule(compiled.content, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
})

function profile(custom = false, status = 'unchecked') {
  return {
    provider: 'openai',
    product: 'Codex Desktop',
    version: '0.155.0-alpha.16.4',
    target: { osType: 'Mac OS', osVersion: '15.7.1', arch: 'arm64', terminal: 'unknown' },
    userAgent: 'same UA text for both explicit selections',
    attributes: [
      { label: '客户端标识', value: 'Codex Desktop; 26.917.71314' },
      { label: '版本策略', value: custom ? '固定自定义' : '跟随官方' },
    ],
    verifiedAt: custom ? undefined : '2026-09-29T00:00:00Z',
    release: {
      status,
      checkedAt: status === 'unchecked' ? undefined : '2026-09-29T01:00:00Z',
      latestVersion: '26.924.51851',
      latestBuild: '12111',
      error: status === 'check_failed' ? 'Codex Desktop artifact does not contain exactly one bundled Core' : undefined,
    },
  }
}

function harness(profiles) {
  const dependencies = {
    'vue': vue,
    '@boxicons/vue': { Openai: {}, Xai: {} },
    '@lucide/vue': Object.fromEntries(['Box', 'CheckCircle2', 'Monitor', 'RefreshCw', 'ShieldCheck', 'Terminal', 'TriangleAlert'].map(name => [name, { name }])),
    '@/components/base/BaseCard.vue': {},
    '@/components/base/BaseEmpty.vue': {},
    '@/components/base/BaseSegmented.vue': {},
    '@/utils/date': { formatDateTime: value => value },
    '@/utils/providers': { formatProviderLabel: value => value },
  }
  const exports = {}
  runInNewContext(outputText, { exports, require: (name) => {
    assert.ok(name in dependencies, name)
    return dependencies[name]
  } })
  const scope = vue.effectScope()
  const props = vue.reactive({ profiles })
  const state = scope.run(() => exports.default.setup(props, { expose() {} }))
  return { props, state, stop: () => scope.stop() }
}

const releaseLabels = {
  unchecked: '待检查',
  aligned: '制品一致',
  review_required: '发现新版',
  check_failed: '检查失败',
}

for (const [status, label] of Object.entries(releaseLabels)) {
  test(`custom identity stays active independently of default release ${status}`, () => {
    const app = harness([profile(true, status)])
    try {
      assert.equal(app.state.identityStatus.value.label, '自定义 UA 生效')
      assert.equal(app.state.identityStatus.value.tone, 'bg-cp-info-container text-cp-info-on-container')
      assert.equal(app.state.verifiedLabel.value, undefined)
      assert.equal(app.state.defaultReleaseStatus.value.label, `默认画像${label}`)
      assert.match(app.state.defaultReleaseStatus.value.title, /默认 Desktop/)
      if (status === 'check_failed')
        assert.match(app.state.defaultReleaseStatus.value.title, /does not contain exactly one bundled Core/)
      if (status !== 'unchecked')
        assert.match(app.state.checkedLabel.value, /^默认画像检查 /)
    }
    finally { app.stop() }
  })

  test(`default identity preserves release ${status}`, () => {
    const app = harness([profile(false, status)])
    try {
      assert.equal(app.state.identityStatus.value.label, label)
      assert.equal(app.state.defaultReleaseStatus.value, null)
      assert.match(app.state.verifiedLabel.value, /^画像核验 /)
    }
    finally { app.stop() }
  })
}

test('explicit selection controls identity even when UA text does not change', () => {
  const app = harness([profile(false, 'check_failed')])
  try {
    const ua = app.state.profile.value.userAgent
    app.props.profiles = [profile(true, 'check_failed')]
    assert.equal(app.state.profile.value.userAgent, ua)
    assert.equal(app.state.identityStatus.value.label, '自定义 UA 生效')
    app.props.profiles = [profile(false, 'aligned')]
    assert.equal(app.state.identityStatus.value.label, '制品一致')
    assert.equal(app.state.defaultReleaseStatus.value, null)
  }
  finally { app.stop() }
})

test('legacy and xAI profiles keep their existing semantics', () => {
  const legacy = profile(false, 'check_failed')
  legacy.attributes = legacy.attributes.filter(item => item.label !== '版本策略')
  const xai = { ...profile(true), provider: 'xai', release: null }
  const app = harness([legacy, xai])
  try {
    assert.equal(app.state.identityStatus.value.label, '检查失败')
    app.state.activeProvider.value = 'xai'
    assert.equal(app.state.identityStatus.value.label, '当前生效')
    assert.equal(app.state.defaultReleaseStatus.value, null)
  }
  finally { app.stop() }
})

test('custom mode without release diagnostics and empty profiles remain valid', () => {
  const app = harness([{ ...profile(true), release: null }])
  try {
    assert.equal(app.state.identityStatus.value.label, '自定义 UA 生效')
    assert.equal(app.state.defaultReleaseStatus.value, null)
    app.props.profiles = []
    assert.equal(app.state.profile.value, null)
    assert.equal(app.state.defaultReleaseStatus.value, null)
  }
  finally { app.stop() }
})
