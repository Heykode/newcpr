import { createApp, h } from 'vue'
import { BaseToast } from '../../src/components/base/BaseToast'
import { applyResolvedTheme, DEFAULT_CUSTOM_THEME_COLOR, DEFAULT_THEME_COLOR, resolveTheme } from '../../src/theme'
import AccountOverviewCards from '../../src/views/accounts/components/AccountOverviewCards.vue'
import NotificationChannelsCard from '../../src/views/settings/components/NotificationChannelsCard.vue'
import { groups } from './monitor-data.mjs'
import '@fontsource-variable/inter'
import '@fontsource-variable/jetbrains-mono'
import '../../src/styles/index.css'

applyResolvedTheme(document.documentElement, resolveTheme(
  new URLSearchParams(location.search).get('theme') === 'dark' ? 'dark' : 'light',
  DEFAULT_THEME_COLOR,
  DEFAULT_CUSTOM_THEME_COLOR,
  {},
))
createApp({
  setup: () => () => h('main', { style: 'max-width:1200px;margin:0 auto;padding:16px;min-width:0' }, [
    h(AccountOverviewCards, {
      summary: { total: 218, normal: 204, quotaExhausted: 5, rateLimited: 3, disabled: 2, error: 4 },
      groups: groups.slice(0, 3),
      groupsLoading: false,
    }),
    h(NotificationChannelsCard),
    h(BaseToast),
  ]),
}).mount('#app')
