import { createApp, h } from 'vue'
import { i18n, setLocale } from '../../src/i18n'
import { applyResolvedTheme, DEFAULT_CUSTOM_THEME_COLOR, DEFAULT_THEME_COLOR, resolveTheme } from '../../src/theme'
import ExcelSettingsCard from '../../src/views/settings/components/ExcelSettingsCard.vue'
import RuntimeSettingsCard from '../../src/views/settings/components/RuntimeSettingsCard.vue'
import { useSettingsForm } from '../../src/views/settings/composables/useSettingsForm'
import '@fontsource-variable/inter'
import '../../src/styles/index.css'

applyResolvedTheme(document.documentElement, resolveTheme('light', DEFAULT_THEME_COLOR, DEFAULT_CUSTOM_THEME_COLOR, {}))
async function mount() {
  await setLocale('zh-Hans')
  createApp({
    setup() {
      const { form, unitFields, excelImages } = useSettingsForm()
      return () => h('main', { style: 'max-width:1100px;margin:0 auto;padding:24px 12px;min-width:0' }, [
        h(RuntimeSettingsCard, {
          'maxConcurrentPerAccount': '5',
          'refreshMarginSeconds': '1800',
          'refreshConcurrency': '4',
          'openaiGuardianReservedConcurrency': String(form.openaiGuardianReservedConcurrency ?? ''),
          'openaiSessionBindingTtlHours': String(form.openaiSessionBindingTtlHours ?? ''),
          'openaiAccountAffinity': form.openaiAccountAffinity,
          'units': unitFields,
          'requestTuning': form.requestTuning,
          'onUpdate:requestTuning': (value) => { form.requestTuning = value },
        }),
        h(ExcelSettingsCard, {
          'models': form.excelDefaultModels,
          'requestTuning': form.requestTuning,
          'images': excelImages,
          'units': unitFields,
          'onUpdate:models': (value) => { form.excelDefaultModels = value },
          'onUpdate:requestTuning': (value) => { form.requestTuning = value },
        }),
        h('output', { 'data-testid': 'saved-bytes' }, String(form.requestTuning.streamPrefetchBytes)),
      ])
    },
  }).use(i18n).mount('#app')
}

void mount()
