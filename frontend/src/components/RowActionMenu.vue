<script setup lang="ts">
import type { ComponentSize } from './base/controls/types'
import { MoreHorizontal } from '@lucide/vue'
import BaseDropdown from './base/BaseDropdown/index.vue'
import BaseIconButton from './base/BaseIconButton.vue'

defineProps<{ label: string, menuClass: string, disabled?: boolean, loading?: boolean, size?: ComponentSize, buttonClass?: string }>()
</script>

<template>
  <BaseDropdown :menu-class="menuClass">
    <template #trigger="{ trigger }">
      <BaseIconButton v-bind="trigger" :size="size ?? 'sm'" :label="label" :disabled="disabled" :loading="loading" :class="[buttonClass, trigger['aria-expanded'] ? 'bg-cp-bg-text-active text-cp-text' : undefined]">
        <MoreHorizontal class="size-4" />
      </BaseIconButton>
    </template>
    <slot />
  </BaseDropdown>
</template>
