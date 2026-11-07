<script setup lang="ts">
import { Info } from '@lucide/vue'

import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BasePopover from '@/components/base/BasePopover.vue'

// 表格单元格的“信息点”悬浮明细：统一触发按钮与面板壳。
defineProps<{
  title: string
  triggerLabel: string
  tone?: 'primary' | 'warning'
}>()
</script>

<template>
  <BasePopover
    trigger="hover"
    placement="right"
  >
    <template #trigger>
      <BaseIconButton
        variant="link"
        :show-tooltip="false"
        class="size-4! rounded-full!"
        :class="tone === 'warning'
          ? 'text-cp-warning-text'
          : 'text-cp-primary-text'"
        :label="triggerLabel"
      >
        <Info class="size-3" />
      </BaseIconButton>
    </template>

    <div class="grid w-60 gap-2 p-3 text-cp-sm leading-none">
      <p class="m-0 font-heavy text-cp-text">
        {{ title }}
      </p>
      <slot />
    </div>
  </BasePopover>
</template>
