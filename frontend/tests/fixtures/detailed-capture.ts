import { createPinia } from 'pinia'
import { createApp, h, ref } from 'vue'
import BaseButton from '../../src/components/base/BaseButton.vue'
import BaseModal from '../../src/components/base/BaseModal/index.vue'
import { applyResolvedTheme, DEFAULT_CUSTOM_THEME_COLOR, DEFAULT_THEME_COLOR, resolveTheme } from '../../src/theme'
import DetailedCaptureControl from '../../src/views/usage/components/DetailedCaptureControl.vue'
import RequestDiagnosticsPanel from '../../src/views/usage/components/RequestDiagnosticsPanel.vue'
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
  setup() {
    const open = ref(false)
    return () => h('main', { style: 'max-width:1100px;margin:auto;padding:24px 12px;min-width:0' }, [
      h('h1', { style: 'font-size:22px;margin:0 0 20px' }, '使用统计'),
      h('div', { style: 'display:flex;align-items:center;flex-wrap:wrap;gap:20px' }, [
        h('h2', { style: 'font-size:18px;margin:0' }, '错误排查'),
        h(DetailedCaptureControl),
      ]),
      h(BaseButton, { style: 'margin-top:24px', onClick: () => open.value = true }, () => '查看错误明细'),
      h(BaseModal, { 'modelValue': open.value, 'onUpdate:modelValue': (value: boolean) => open.value = value, 'title': '错误明细', 'size': 'xl' }, {
        default: () => open.value ? h(RequestDiagnosticsPanel, { requestId: 'req_capture_fixture' }) : null,
      }),
    ])
  },
}).use(createPinia()).mount('#app')
