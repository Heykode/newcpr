<script setup lang="ts">
import type { CSSProperties } from 'vue'
import type { ThemeEditorPreview } from '../composables/useThemeEditor'
import type { ThemeName } from '@/theme'
import { defineAsyncComponent, onBeforeUnmount, onMounted, shallowRef } from 'vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseSegmented from '@/components/base/BaseSegmented.vue'

import { t } from '@/i18n'

import ThemePreviewCanvas from './ThemePreviewCanvas.vue'
import ThemePreviewError from './ThemePreviewError.vue'

defineProps<{
  theme: ThemeName
  style: CSSProperties
}>()

const previewReady = shallowRef(false)
const stylesRevision = shallowRef(0)
let paintFrame = 0

// 先绘制画布和控件，再挂载预览；缓存命中时也不阻塞外框首帧
onMounted(() => {
  paintFrame = requestAnimationFrame(() => {
    paintFrame = requestAnimationFrame(() => {
      previewReady.value = true
    })
  })
})
onBeforeUnmount(() => cancelAnimationFrame(paintFrame))

const ThemeDashboardPreview = defineAsyncComponent({
  loader: async () => {
    const component = await import('./ThemeDashboardPreview.vue')
    stylesRevision.value++
    return component
  },
  errorComponent: ThemePreviewError,
  timeout: 30000,
})
const ThemeComponentPreview = defineAsyncComponent({
  loader: async () => {
    const component = await import('./ThemeComponentPreview.vue')
    stylesRevision.value++
    return component
  },
  errorComponent: ThemePreviewError,
  timeout: 30000,
})

const preview = defineModel<ThemeEditorPreview>({ required: true })

const previewOptions = [
  { label: '首页画板', value: 'page' },
  { label: '组件概览', value: 'components' },
]
</script>

<template>
  <section class="grid h-full min-h-0 grid-rows-[auto_minmax(0,1fr)] gap-3 overflow-hidden" :aria-label="t('appearance.preview.themeRealTimePreview')">
    <BaseCard as="header" padding="none" class="shrink-0 flex flex-wrap items-center justify-between gap-3 px-3 py-2.5">
      <div>
        <strong class="block text-cp-sm font-heavy text-cp-text">{{ t('appearance.preview.livePreview') }}</strong>
        <span class="mt-0.5 block text-[9px] font-emphasis text-cp-text-quaternary">{{ t('appearance.preview.description') }}</span>
      </div>
      <BaseSegmented v-model="preview" :label="t('appearance.preview.previewType')" size="sm" :options="previewOptions" />
    </BaseCard>

    <ThemePreviewCanvas
      class="h-full min-h-0"
      :theme="theme"
      :style="style"
      :styles-revision="stylesRevision"
    >
      <template v-if="previewReady">
        <ThemeDashboardPreview v-if="preview === 'page'" />
        <ThemeComponentPreview v-else />
      </template>
    </ThemePreviewCanvas>
  </section>
</template>
