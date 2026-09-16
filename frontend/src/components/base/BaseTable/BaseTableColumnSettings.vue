<script setup lang="ts">
import type { TableColumnOption } from './useTableColumns'
import { Columns3, RotateCcw } from '@lucide/vue'
import { nextTick, shallowRef, useId, useTemplateRef, watch } from 'vue'
import BaseButton from '../BaseButton.vue'
import BaseCheckbox from '../BaseCheckbox.vue'
import BaseIconButton from '../BaseIconButton.vue'
import BasePopover from '../BasePopover.vue'
import BaseScrollbar from '../BaseScrollbar.vue'

defineProps<{
  options: TableColumnOption[]
  label: string
}>()

const emit = defineEmits<{
  change: [key: string, visible: boolean]
  reset: []
}>()

const open = shallowRef(false)
const panelId = useId()
const triggerRef = useTemplateRef<InstanceType<typeof BaseIconButton>>('trigger')
const panelRef = useTemplateRef<HTMLDivElement>('panel')

function controls() {
  return panelRef.value?.querySelectorAll<HTMLElement>('input:not(:disabled), button:not(:disabled)')
}

function closeAndFocus() {
  open.value = false
  triggerRef.value?.$el.focus()
}

function handleTab(event: KeyboardEvent) {
  const elements = controls()
  if (!elements?.length)
    return

  const edge = event.shiftKey ? elements[0] : elements[elements.length - 1]
  if (document.activeElement !== edge)
    return

  // The teleported panel returns to the trigger's toolbar tab order.
  if (event.shiftKey)
    event.preventDefault()
  closeAndFocus()
}

function handleFocusOut(event: FocusEvent) {
  if (event.relatedTarget instanceof Node
    && !panelRef.value?.contains(event.relatedTarget)
    && !triggerRef.value?.$el.contains(event.relatedTarget)) {
    open.value = false
  }
}

watch(open, async (value) => {
  if (!value)
    return
  await nextTick()
  if (open.value)
    controls()?.[0]?.focus({ preventScroll: true })
})
</script>

<template>
  <BasePopover
    v-model="open"
    placement="bottom-end"
    :arrow-surface-class="{ top: 'bg-(--cp-popover-header-bg)' }"
  >
    <template #trigger>
      <BaseIconButton
        ref="trigger"
        :label="label"
        variant="ghost"
        :pressed="open"
        aria-haspopup="dialog"
        :aria-expanded="open"
        :aria-controls="open ? panelId : undefined"
      >
        <Columns3 class="size-4.5" aria-hidden="true" />
      </BaseIconButton>
    </template>

    <div
      role="presentation"
      @keydown.esc.stop.prevent="closeAndFocus"
      @keydown.tab="handleTab"
      @focusout="handleFocusOut"
    >
      <div
        :id="panelId"
        ref="panel"
        role="dialog"
        :aria-label="label"
        class="w-60 max-w-full overflow-hidden rounded-cp-lg"
      >
        <div class="flex items-center justify-between gap-3 bg-cp-popover-header-bg px-3 py-2.5">
          <span class="text-cp-sm font-bold text-cp-text">显示列</span>
          <span class="text-cp-xs text-cp-text-secondary">{{ options.filter(option => option.visible).length }} / {{ options.length }}</span>
        </div>
        <BaseScrollbar max-height="min(20rem, calc(100dvh - 10rem))">
          <div class="grid gap-0.5 p-2">
            <BaseCheckbox
              v-for="option in options"
              :key="option.key"
              :model-value="option.visible"
              :label="option.label"
              :disabled="option.disabled"
              show-label
              class="min-h-9 w-full rounded-cp px-2 py-2"
              :class="option.disabled ? undefined : 'hover:bg-cp-bg-text-hover'"
              @update:model-value="emit('change', option.key, $event)"
            />
          </div>
        </BaseScrollbar>
        <div class="flex justify-end px-2 pb-2">
          <BaseButton variant="ghost" size="sm" @click="emit('reset')">
            <RotateCcw class="size-3.5" aria-hidden="true" />
            恢复默认
          </BaseButton>
        </div>
      </div>
    </div>
  </BasePopover>
</template>
