<script setup lang="ts">
import { CircleAlert } from '@lucide/vue'
import BaseIconButton from './base/BaseIconButton.vue'
import BasePopover from './base/BasePopover.vue'

defineProps<{ label: string, content: string }>()
</script>

<template>
  <BasePopover class="h-4 items-center" trigger="hover-click" placement="top-start" :hover-delay="240">
    <template #trigger="{ open }">
      <BaseIconButton variant="link" size="sm" :label="label" :aria-expanded="open" aria-haspopup="dialog" class="size-4!">
        <CircleAlert class="size-3.5" aria-hidden="true" />
      </BaseIconButton>
    </template>
    <section role="dialog" :aria-label="label" class="max-w-72 whitespace-pre-line px-3 py-2 text-cp-sm leading-relaxed text-cp-text-secondary">
      {{ content }}
    </section>
  </BasePopover>
</template>
