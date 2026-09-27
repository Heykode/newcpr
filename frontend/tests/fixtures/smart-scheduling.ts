import type { RotationStrategy } from '../../src/api/modules/settings'
import { createApp, h, ref } from 'vue'
import { defaultSmartScheduling } from '../../src/api/modules/settings'
import { applyResolvedTheme, DEFAULT_CUSTOM_THEME_COLOR, DEFAULT_THEME_COLOR, resolveTheme } from '../../src/theme'
import RotationStrategyCard from '../../src/views/settings/components/RotationStrategyCard.vue'
import { rotationOptions } from '../../src/views/settings/constants'
import '@fontsource-variable/inter'
import '../../src/styles/index.css'

applyResolvedTheme(document.documentElement, resolveTheme('light', DEFAULT_THEME_COLOR, DEFAULT_CUSTOM_THEME_COLOR, {}))
createApp({
  setup() {
    const strategy = ref<RotationStrategy>('sticky')
    const smart = ref(defaultSmartScheduling())
    return () => h('main', { style: 'max-width:1100px;margin:0 auto;padding:24px 12px;min-width:0' }, [
      h(RotationStrategyCard, {
        'modelValue': strategy.value,
        'onUpdate:modelValue': (value) => {
          if (value)
            strategy.value = value
        },
        'smartScheduling': smart.value,
        'onUpdate:smartScheduling': (value) => { smart.value = value },
        'options': rotationOptions,
      }),
    ])
  },
}).mount('#app')
