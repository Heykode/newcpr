import type { Locale } from './i18n/locale'
import { createApp } from 'vue'
import App from './App.vue'
import { loading } from './directives/loading'

import { i18n, locale, setLocale } from './i18n'
import { authPlugin } from './plugins/auth'
import { router } from './router'
import { pinia } from './stores'
import { useThemeStore } from './stores/modules/theme'
import '@fontsource-variable/inter'
import '@fontsource-variable/jetbrains-mono'

import './styles/index.css'

const app = createApp(App)

app.directive('loading', loading)
app.use(pinia)
app.use(i18n)

useThemeStore(pinia).initializeTheme()

app.use(router)
app.use(authPlugin)
void setLocale(locale.value as Locale).catch(() => setLocale('zh-Hans')).finally(() => app.mount('#app'))
