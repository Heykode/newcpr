<script setup lang="ts">
import { Funnel } from '@lucide/vue'
import BasePopover from '@/components/base/BasePopover.vue'

defineProps<{ label: string, active?: boolean }>()
</script>

<template>
  <BasePopover trigger="click" placement="bottom-start">
    <template #trigger>
      <button type="button" class="inline-flex items-center gap-1.5 whitespace-nowrap py-1 text-left font-medium hover:text-cp-primary" :class="active ? 'text-cp-primary' : 'text-cp-text-secondary'" :aria-label="`${label}筛选`">
        {{ label }}<Funnel class="size-3.5" :class="active ? 'fill-cp-primary/15' : ''" />
      </button>
    </template>
    <div class="grid max-h-[min(32rem,70vh)] w-64 max-w-[calc(100vw-2rem)] gap-3 overflow-y-auto p-3" :aria-label="`${label}筛选条件`">
      <slot />
    </div>
  </BasePopover>
</template>
