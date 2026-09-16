import { createApp, h } from 'vue'
import { applyResolvedTheme, DEFAULT_CUSTOM_THEME_COLOR, DEFAULT_THEME_COLOR, resolveTheme } from '../../src/theme'
import OutboundUserAgentCard from '../../src/views/settings/components/OutboundUserAgentCard.vue'
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
  setup: () => () => h('main', { style: 'max-width:1000px;margin:0 auto;padding:24px 16px;min-width:0' }, [
    h(OutboundUserAgentCard),
  ]),
}).mount('#app')
