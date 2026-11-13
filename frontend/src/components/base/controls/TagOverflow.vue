<script setup lang="ts" generic="Value extends string | number">
import { useEventListener } from '@vueuse/core'
import { nextTick, ref, useId, watch } from 'vue'
import Popover from '../BasePopover.vue'
import Scrollbar from '../BaseScrollbar.vue'
import Tag from '../BaseTag.vue'
import { inputTagClass } from './input-styles'

const props = withDefaults(defineProps<{
  items: { value: Value, label: string, disabled?: boolean }[]
  size: 'sm' | 'md'
  tooltip: boolean
  moreLabel: string
  panelLabel: string
  closeLabel?: (label: string) => string
  fallbackTitle?: string
  disabled?: boolean
  readonly?: boolean
  blocked?: boolean
  teleported?: boolean
}>(), { teleported: true })

const emit = defineEmits<{ remove: [value: Value], focusControl: [] }>()
const open = ref(false)
const trigger = ref<HTMLButtonElement | null>(null)
const panelId = useId()
const panel = ref<HTMLElement | null>(null)
async function remove(value: Value, event: MouseEvent) {
  const returnFocus = props.items.length === 1
  emit('remove', value)
  // 键盘移除后将焦点交还仍存在的控件，鼠标连续移除保持浮层。
  if (returnFocus || event.detail === 0) {
    await nextTick()
    if (open.value && props.items.length)
      trigger.value?.focus()
    else emit('focusControl')
  }
}
async function focusHidden() {
  if (props.disabled || props.blocked)
    return
  open.value = true
  await nextTick()
  const target = panel.value?.querySelector<HTMLButtonElement>('button') ?? panel.value
  target?.focus()
}
function close(event: KeyboardEvent) {
  if (!open.value)
    return
  event.preventDefault()
  event.stopPropagation()
  open.value = false
  emit('focusControl')
}
useEventListener(panel, 'keydown', (event) => {
  if (event.key === 'Escape')
    close(event)
})
watch(() => [props.blocked, props.disabled, props.tooltip, props.items.length], () => {
  if (props.blocked || props.disabled || !props.tooltip || !props.items.length)
    open.value = false
})
</script>

<template>
  <Popover
    v-if="items.length && tooltip"
    v-model="open"
    trigger="hover-click"
    placement="bottom"
    :disabled="disabled || blocked"
    :teleported="teleported"
    class="pointer-events-auto shrink-0"
  >
    <template #trigger>
      <button
        ref="trigger"
        type="button"
        :disabled="disabled"
        :aria-label="moreLabel"
        :aria-expanded="open"
        :aria-controls="open ? panelId : undefined"
        aria-haspopup="dialog"
        class="inline-flex items-center cursor-pointer rounded-cp border-0 bg-transparent p-0 outline-none focus-visible:ring-2 focus-visible:ring-cp-control-outline disabled:cursor-not-allowed"
        @keydown.esc="close"
        @keydown.enter.prevent="focusHidden"
        @keydown.space.prevent="focusHidden"
        @keydown.down.prevent="focusHidden"
      >
        <Tag :size="size" :disabled="disabled" :class="inputTagClass" class="tabular-nums">
          +{{ items.length }}
        </Tag>
      </button>
    </template>
    <div :id="panelId" ref="panel" role="dialog" tabindex="-1" :aria-label="panelLabel" class="max-w-[min(20rem,calc(100vw-2rem))] p-2">
      <Scrollbar max-height="12rem">
        <div class="flex flex-wrap gap-1">
          <Tag
            v-for="option in items"
            :key="option.value"
            :class="inputTagClass"
            :size="size"
            :closable="!disabled && !readonly && !option.disabled"
            :close-label="closeLabel?.(option.label)"
            :disabled="disabled"
            @close="remove(option.value, $event)"
          >
            <span class="block max-w-56 truncate" :title="option.label">{{ option.label }}</span>
          </Tag>
        </div>
      </Scrollbar>
    </div>
  </Popover>
  <Tag v-else-if="items.length" class="shrink-0 tabular-nums" :size="size" :class="inputTagClass" :disabled="disabled" :title="fallbackTitle">
    +{{ items.length }}
  </Tag>
</template>
