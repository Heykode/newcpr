import type { DashboardWireProfile } from '../../src/api/modules/dashboard'
import { createApp, h } from 'vue'
import { applyResolvedTheme, DEFAULT_CUSTOM_THEME_COLOR, DEFAULT_THEME_COLOR, resolveTheme } from '../../src/theme'
import WireProfileCard from '../../src/views/dashboard/components/WireProfileCard.vue'
import '@fontsource-variable/inter'
import '@fontsource-variable/jetbrains-mono'
import '../../src/styles/index.css'

applyResolvedTheme(document.documentElement, resolveTheme(
  new URLSearchParams(location.search).get('theme') === 'dark' ? 'dark' : 'light',
  DEFAULT_THEME_COLOR,
  DEFAULT_CUSTOM_THEME_COLOR,
  {},
))

const baseline: DashboardWireProfile = {
  provider: 'openai',
  product: 'Codex Desktop',
  version: '0.155.0-alpha.16.4',
  target: { osType: 'Mac OS', osVersion: '15.7.1', arch: 'arm64', terminal: 'unknown' },
  userAgent: 'Codex Desktop/0.155.0-alpha.16.4 (Mac OS 15.7.1; arm64) unknown (Codex Desktop; 26.917.71314)',
  attributes: [
    { label: '客户端标识', value: 'Codex Desktop; 26.917.71314' },
    { label: '版本策略', value: '跟随官方' },
  ],
  verifiedAt: '2026-09-29T00:00:00Z',
}
const custom: DashboardWireProfile = {
  ...baseline,
  product: 'codex_exec',
  version: '0.156.1',
  userAgent: 'codex_exec/0.156.1 (Mac OS 15.7.9; arm64) xterm-256color (codex_exec; 0.156.1)',
  attributes: [
    { label: '客户端标识', value: 'codex_exec; 0.156.1' },
    { label: '版本策略', value: '固定自定义' },
  ],
  verifiedAt: undefined,
}
const failed: DashboardWireProfile['release'] = {
  status: 'check_failed',
  checkedAt: '2026-09-29T01:00:00Z',
  error: 'Codex Desktop artifact does not contain exactly one bundled Core',
}
const aligned: DashboardWireProfile['release'] = {
  status: 'aligned',
  checkedAt: '2026-09-29T01:00:00Z',
  latestVersion: '26.924.51851',
  latestBuild: '12111',
}
const cases = [
  { name: 'custom-failed', profile: { ...custom, release: failed } },
  { name: 'default-failed', profile: { ...baseline, release: failed } },
  { name: 'custom-aligned', profile: { ...custom, release: aligned } },
  { name: 'default-aligned', profile: { ...baseline, release: aligned } },
]
createApp({
  setup: () => () => h('main', {
    style: 'max-width:1200px;margin:0 auto;padding:16px;display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,400px),1fr));gap:16px;min-width:0',
  }, cases.map(item => h(WireProfileCard, {
    'data-case': item.name,
    'profiles': [item.profile],
  }))),
}).mount('#app')
