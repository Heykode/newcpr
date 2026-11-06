<script setup lang="ts">
import { shallowRef, useTemplateRef } from 'vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BasePopover from '@/components/base/BasePopover.vue'

defineProps<{
  label: string
  disabled?: boolean
}>()

const selected = defineModel<boolean>({ required: true })
const labelText = useTemplateRef<HTMLSpanElement>('labelText')
const open = shallowRef(false)

function updateOpen(nextOpen: boolean) {
  const text = labelText.value
  open.value = nextOpen && Boolean(text && text.scrollWidth > text.clientWidth)
}

function dismissTooltip(event: KeyboardEvent, close: () => void) {
  if (!open.value)
    return
  event.preventDefault()
  event.stopPropagation()
  close()
}
</script>

<template>
  <BasePopover :model-value="open" class="min-w-0 flex-1" trigger="hover" placement="top" @update:model-value="updateOpen">
    <template #trigger="{ close }">
      <BaseCheckbox v-model="selected" class="min-h-11 min-w-0 flex-1 px-3 py-2.5" show-label :label="label" :disabled="disabled" @keydown.esc="dismissTooltip($event, close)">
        <template #label>
          <span ref="labelText" class="block truncate">{{ label }}</span>
        </template>
      </BaseCheckbox>
    </template>
    <div class="max-w-96 px-3 py-2 text-cp-sm wrap-anywhere">
      {{ label }}
    </div>
  </BasePopover>
</template>
