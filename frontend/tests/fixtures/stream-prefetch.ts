import { createApp, h } from 'vue'
import { applyResolvedTheme, DEFAULT_CUSTOM_THEME_COLOR, DEFAULT_THEME_COLOR, resolveTheme } from '../../src/theme'
import RuntimeSettingsCard from '../../src/views/settings/components/RuntimeSettingsCard.vue'
import { useSettingsForm } from '../../src/views/settings/composables/useSettingsForm'
import '@fontsource-variable/inter'
import '../../src/styles/index.css'

applyResolvedTheme(document.documentElement, resolveTheme('light', DEFAULT_THEME_COLOR, DEFAULT_CUSTOM_THEME_COLOR, {}))
createApp({
  setup() {
    const { form } = useSettingsForm()
    return () => h('main', { style: 'max-width:1100px;margin:0 auto;padding:24px 12px;min-width:0' }, [
      h(RuntimeSettingsCard, {
        'maxConcurrentPerAccount': '5',
        'refreshMarginSeconds': '1800',
        'refreshConcurrency': '4',
        'requestIntervalMs': '25',
        'responsesMaxDecompressedBodyBytes': '67108864',
        'disableFast': false,
        'requestTuning': form.requestTuning,
        'onUpdate:requestTuning': (value) => { form.requestTuning = value },
      }),
      h('output', { 'data-testid': 'saved-bytes' }, String(form.requestTuning.streamPrefetchBytes)),
    ])
  },
}).mount('#app')
