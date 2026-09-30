import { createPinia } from 'pinia'
import { createApp, h } from 'vue'
import { applyResolvedTheme, DEFAULT_CUSTOM_THEME_COLOR, DEFAULT_THEME_COLOR, resolveTheme } from '../../src/theme'
import SettingsCleanupSection from '../../src/views/settings/components/cleanup/SettingsCleanupSection.vue'
import '@fontsource-variable/inter'
import '@fontsource-variable/jetbrains-mono'
import '../../src/styles/index.css'

applyResolvedTheme(document.documentElement, resolveTheme('light', DEFAULT_THEME_COLOR, DEFAULT_CUSTOM_THEME_COLOR, {}))
createApp({
  render: () => h('main', { style: 'max-width:1100px;margin:auto;padding:24px 12px;min-width:0' }, [
    h('h1', { style: 'font-size:22px;margin:0 0 24px' }, '系统设置'),
    h(SettingsCleanupSection),
  ]),
}).use(createPinia()).mount('#app')
