import { createApp, h } from 'vue'
import { applyResolvedTheme, DEFAULT_CUSTOM_THEME_COLOR, DEFAULT_THEME_COLOR, resolveTheme } from '../../src/theme'
import AccountOverviewCards from '../../src/views/accounts/components/AccountOverviewCards.vue'
import { groups } from './monitor-data.mjs'
import '@fontsource-variable/inter'
import '@fontsource-variable/jetbrains-mono'
import '../../src/styles/index.css'

const query = new URLSearchParams(location.search)
const fixtureCount = Number(query.get('count'))
const fixtureGroups = query.has('onlyEmpty')
  ? groups.filter(group => group.memberCount === 0)
  : query.has('count') && Number.isInteger(fixtureCount) && fixtureCount >= 1 && fixtureCount <= 6
    ? groups.slice(0, fixtureCount)
    : groups
applyResolvedTheme(document.documentElement, resolveTheme(
  new URLSearchParams(location.search).get('theme') === 'dark' ? 'dark' : 'light',
  DEFAULT_THEME_COLOR,
  DEFAULT_CUSTOM_THEME_COLOR,
  {},
))
createApp({
  setup: () => () => h('main', { style: 'max-width:1200px;margin:0 auto;padding:24px 16px;min-width:0' }, [
    h('h1', { style: 'font-size:22px;font-weight:650;margin:0;letter-spacing:0' }, '账号管理 · 本地样例'),
    h(AccountOverviewCards, {
      summary: { total: 218, normal: 204, quotaExhausted: 5, rateLimited: 3, disabled: 2, error: 4 },
      groups: fixtureGroups,
      groupsLoading: false,
    }),
  ]),
}).mount('#app')
