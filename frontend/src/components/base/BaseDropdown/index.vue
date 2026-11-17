<script setup lang="ts">
import type { PopoverPlacement } from '../BasePopover/types'
import { computed, nextTick, provide, shallowRef, useId, useTemplateRef } from 'vue'
import Popover from '../BasePopover.vue'
import { dropdownSelectKey } from './context'

defineProps<{ placement?: PopoverPlacement, menuClass?: string }>()
defineSlots<{ trigger: (props: { trigger: typeof trigger.value }) => unknown, default: () => unknown }>()

const open = shallowRef(false)
const menu = useTemplateRef('menu')
const popover = useTemplateRef('popover')
const triggerId = useId()
const menuId = useId()

function items() {
  return Array.from(menu.value?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)') ?? [])
}

async function focusItem(last = false) {
  await nextTick()
  if (!open.value)
    return
  const target = items().at(last ? -1 : 0) ?? menu.value
  target?.focus()
}

function closeAndFocus() {
  if (!open.value)
    return
  open.value = false
  popover.value?.focus()
}

provide(dropdownSelectKey, closeAndFocus)

const trigger = computed(() => ({
  'id': triggerId,
  'aria-haspopup': 'menu' as const,
  'aria-expanded': open.value,
  'aria-controls': open.value ? menuId : undefined,
  onKeydown(event: KeyboardEvent) {
    if (event.key === 'Tab')
      open.value = false
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      open.value = true
      void focusItem(event.key === 'ArrowUp')
    }
  },
  onClick(event: MouseEvent) {
    // 原生按钮的键盘激活会产生 detail 为零的 click，鼠标打开时保留原焦点
    if (event.detail === 0) {
      event.stopPropagation()
      open.value = !open.value
      if (open.value)
        void focusItem()
    }
  },
}))

function navigate(event: KeyboardEvent) {
  if (event.key === 'Escape' || event.key === 'Tab') {
    if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
    }
    closeAndFocus()
    return
  }

  const choices = items()
  if (!choices.length)
    return
  const current = choices.indexOf(document.activeElement as HTMLButtonElement)
  let index: number
  switch (event.key) {
    case 'Home':
      index = 0
      break
    case 'End':
      index = choices.length - 1
      break
    case 'ArrowDown':
      index = (current + 1) % choices.length
      break
    case 'ArrowUp':
      index = current <= 0 ? choices.length - 1 : current - 1
      break
    default: return
  }
  event.preventDefault()
  choices[index]?.focus()
}
</script>

<template>
  <Popover ref="popover" v-model="open" :placement="placement">
    <template #trigger>
      <slot name="trigger" :trigger="trigger" />
    </template>
    <div :id="menuId" ref="menu" role="menu" :aria-labelledby="triggerId" tabindex="-1" :class="menuClass" @keydown="navigate">
      <slot />
    </div>
  </Popover>
</template>
