<script setup lang="ts">
import { X } from '@lucide/vue'
import { nextTick, onBeforeUnmount, ref, useId, watch } from 'vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import { lockBodyScroll, unlockBodyScroll } from '@/components/base/BaseModal/bodyScrollLock'

const props = defineProps<{ title: string, busy?: boolean }>()
const open = defineModel<boolean>({ default: false })
const panel = ref<HTMLElement>()
const titleId = useId()
let previousFocus: HTMLElement | null = null
let locked = false

function close() {
  if (!props.busy)
    open.value = false
}
function focusable() {
  return Array.from(panel.value?.querySelectorAll<HTMLElement>('button:not([disabled]),input:not([disabled]),textarea:not([disabled]),[tabindex="0"]') ?? [])
}
function keydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault()
    close()
  }
  if (event.key !== 'Tab')
    return
  const items = focusable()
  const first = items[0]
  const last = items.at(-1)
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault()
    last?.focus()
  }
  else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault()
    first?.focus()
  }
}
function unlock() {
  if (locked) {
    unlockBodyScroll()
    locked = false
  }
}
watch(open, async (value) => {
  if (value) {
    previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null
    if (!locked) {
      lockBodyScroll()
      locked = true
    }
    await nextTick()
    if (open.value)
      (focusable()[0] ?? panel.value)?.focus()
  }
  else {
    unlock()
    if (previousFocus?.isConnected)
      previousFocus.focus()
    previousFocus = null
  }
}, { immediate: true })
onBeforeUnmount(unlock)
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="fixed inset-0 z-50" role="presentation" @keydown="keydown">
      <button type="button" class="absolute inset-0 cursor-default border-0 bg-cp-bg-mask" tabindex="-1" aria-label="关闭侧栏" @click="close" />
      <section ref="panel" role="dialog" aria-modal="true" tabindex="-1" :aria-labelledby="titleId" class="quality-drawer bg-cp-bg-container text-cp-text">
        <div class="flex h-full min-h-0 flex-col">
          <header class="flex shrink-0 items-center justify-between gap-3 border-b border-cp-border p-5">
            <h2 :id="titleId" class="min-w-0 text-lg font-semibold">
              {{ title }}
            </h2>
            <BaseIconButton label="关闭" :disabled="busy" @click="open = false">
              <X class="size-4" />
            </BaseIconButton>
          </header>
          <div class="min-h-0 flex-1 overflow-y-auto p-5">
            <slot />
          </div>
          <footer v-if="$slots.footer" class="flex shrink-0 justify-end gap-3 border-t border-cp-border p-5">
            <slot name="footer" />
          </footer>
        </div>
      </section>
    </div>
  </Teleport>
</template>

<style scoped>
.quality-drawer {
  position: fixed;
  inset: 0 0 0 auto;
  width: min(640px, 100%);
  max-width: 100%;
  height: 100dvh;
  max-height: 100dvh;
  margin: 0 0 0 auto;
  border: 0;
  padding: 0;
  letter-spacing: 0;
}
</style>
