<script setup lang="ts">
import type { CSSProperties } from 'vue'
import type { ThemeEditorPreview } from '../composables/useThemeEditor'
import type { ThemeName } from '@/theme'
import BaseSegmented from '@/components/base/BaseSegmented.vue'

import { t } from '@/i18n'

import ThemeComponentPreview from './ThemeComponentPreview.vue'
import ThemeDashboardPreview from './ThemeDashboardPreview.vue'
import ThemePreviewCanvas from './ThemePreviewCanvas.vue'

defineProps<{
  theme: ThemeName
  style: CSSProperties
}>()

const preview = defineModel<ThemeEditorPreview>({ required: true })

const previewOptions = [
  { label: '首页画板', value: 'page' },
  { label: '组件概览', value: 'components' },
]
</script>

<template>
  <section class="grid h-full min-h-0 grid-rows-[auto_minmax(0,1fr)] gap-3 overflow-hidden" :aria-label="t('appearance.preview.themeRealTimePreview')">
    <header class="shrink-0 flex flex-wrap items-center justify-between gap-3 rounded-cp-lg bg-cp-bg-container px-3 py-2.5 shadow-cp-tertiary">
      <div>
        <strong class="block text-cp-sm font-heavy text-cp-text">{{ t('appearance.preview.livePreview') }}</strong>
        <span class="mt-0.5 block text-[9px] font-emphasis text-cp-text-quaternary">{{ t('appearance.preview.description') }}</span>
      </div>
      <BaseSegmented v-model="preview" :label="t('appearance.preview.previewType')" size="sm" :options="previewOptions" />
    </header>

    <ThemePreviewCanvas
      class="h-full min-h-0"
      :theme="theme"
      :style="style"
    >
      <ThemeDashboardPreview v-if="preview === 'page'" />
      <ThemeComponentPreview v-else />
    </ThemePreviewCanvas>
  </section>
</template>
