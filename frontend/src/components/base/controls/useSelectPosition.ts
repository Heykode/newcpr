import type { CSSProperties, Ref } from 'vue'
import { useEventListener, useResizeObserver } from '@vueuse/core'
import { clamp } from 'es-toolkit'
import { computed, ref, shallowRef } from 'vue'

export interface SelectPositionElements {
  controlRef: Ref<HTMLElement | null>
  listboxRef: Ref<HTMLElement | null>
}

export function useSelectPosition(open: Ref<boolean>, selectId: string, elements: SelectPositionElements) {
  const { controlRef, listboxRef } = elements
  const popoverStyle = shallowRef<CSSProperties>({})
  const popoverMaxHeight = ref('244px')
  const popoverPlacement = ref<'top' | 'bottom'>('bottom')

  function updatePopoverPosition() {
    if (!open.value || !controlRef.value || !listboxRef.value)
      return

    const rect = controlRef.value.getBoundingClientRect()
    const gap = 6
    const menuHeight = Math.min(listboxRef.value.scrollHeight, 244)
    const belowSpace = window.innerHeight - rect.bottom - gap - 8
    const aboveSpace = rect.top - gap - 8
    const placeAbove = belowSpace < menuHeight && aboveSpace > belowSpace
    const availableHeight = Math.max(placeAbove ? aboveSpace : belowSpace, 0)
    const maxHeight = Math.min(menuHeight, availableHeight)
    const top = placeAbove
      ? Math.max(8, rect.top - maxHeight - gap)
      : Math.min(rect.bottom + gap, window.innerHeight - maxHeight - 8)
    const left = clamp(rect.left, 8, window.innerWidth - rect.width - 8)

    // 原生锚点由浏览器随滚动合成，避免 body 浮层等待主线程坐标更新。
    const nativeAnchor = CSS.supports('top', 'anchor(bottom)')
    popoverPlacement.value = placeAbove ? 'top' : 'bottom'
    popoverMaxHeight.value = `${maxHeight}px`
    popoverStyle.value = {
      positionAnchor: nativeAnchor ? `--${selectId}` : undefined,
      positionVisibility: nativeAnchor ? 'anchors-visible' : undefined,
      left: nativeAnchor ? 'anchor(left)' : `${left}px`,
      top: nativeAnchor
        ? placeAbove ? `calc(anchor(top) - ${maxHeight + gap}px)` : `calc(anchor(bottom) + ${gap}px)`
        : `${top}px`,
      width: nativeAnchor ? 'anchor-size(width)' : `${rect.width}px`,
      maxWidth: 'calc(100vw - 16px)',
    }
  }

  // 滚动事件随浏览器绘制更新，额外节流会让固定定位的弹层落后于输入框。
  const viewportTarget = computed(() => open.value ? window : null)
  useEventListener(viewportTarget, 'resize', updatePopoverPosition)
  useEventListener(viewportTarget, 'scroll', updatePopoverPosition, { capture: true, passive: true })
  useResizeObserver([controlRef, listboxRef], updatePopoverPosition)

  return { popoverStyle, popoverMaxHeight, popoverPlacement, updatePopoverPosition }
}
