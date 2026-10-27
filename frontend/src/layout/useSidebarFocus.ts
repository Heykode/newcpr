import type { Ref } from 'vue'
import { useEventListener, useMediaQuery } from '@vueuse/core'
import { nextTick, onScopeDispose, watch } from 'vue'

export function useSidebarFocus(panel: Ref<HTMLElement | null>, open: Ref<boolean>) {
  let previous: HTMLElement | null = null
  const desktop = useMediaQuery('(min-width: 961px)')
  const focusable = () => Array.from(panel.value?.querySelectorAll<HTMLElement>(
    'a[href],button:not([disabled]),input:not([disabled]),[tabindex]:not([tabindex="-1"])',
  ) ?? []).filter(element => element.getClientRects().length && !element.closest('[inert]'))
  function restoreFocus() {
    const active = document.activeElement
    if (previous?.isConnected && previous.getClientRects().length && !open.value
      && (active === document.body || panel.value?.contains(active))) {
      previous.focus()
    }
    previous = null
  }
  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault()
      open.value = false
    }
    if (event.key !== 'Tab')
      return
    const items = focusable()
    const first = items[0]
    const last = items.at(-1)
    if (!first || !panel.value?.contains(document.activeElement)) {
      event.preventDefault()
      ;(event.shiftKey ? last : first)?.focus()
      if (!first)
        panel.value?.focus()
    }
    else if (event.shiftKey && document.activeElement === first) {
      event.preventDefault()
      last?.focus()
    }
    else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault()
      first.focus()
    }
  }
  watch(open, async (value) => {
    if (!value)
      return
    previous = document.activeElement instanceof HTMLElement ? document.activeElement : null
    await nextTick()
    if (open.value)
      (focusable()[0] ?? panel.value)?.focus()
  })
  watch(desktop, (value) => {
    if (value)
      open.value = false
  })
  useEventListener(panel, 'keydown', handleKeydown)
  onScopeDispose(restoreFocus)
  return { restoreFocus }
}
