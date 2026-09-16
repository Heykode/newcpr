import type { CSSProperties, Ref } from 'vue'
import { onScopeDispose, shallowRef, toRaw, watch } from 'vue'

export interface AccountSwipeSelectOptions {
  getScrollElement: () => HTMLElement | undefined
  getTableElement: () => HTMLTableElement | undefined
  rowIds: Readonly<Ref<string[]>>
  selectedIds: Ref<Set<string>>
  disabled: Readonly<Ref<boolean>>
  invalidationKey?: Readonly<Ref<unknown>>
}

const DRAG_THRESHOLD = 5
const SCROLL_ZONE = 60
const SCROLL_STEP = 8
const ignoredSelector = [
  '[data-swipe-select-ignore]',
  'button',
  'input',
  'select',
  'textarea',
  'a',
  'label',
  'summary',
  'option',
  '[contenteditable]:not([contenteditable="false"])',
  '[draggable="true"]',
  '[role="button"]',
  '[role="checkbox"]',
  '[role="link"]',
  '[role="menuitem"]',
  '[role="combobox"]',
  '[role="slider"]',
  '[role="switch"]',
].join(', ')

interface DragSession {
  document: Document
  window: Window
  ids: string[]
  initialSelection: Set<string>
  selectionOwner: Set<string>
  expectedSelection: Set<string>
  startIndex: number
  startY: number
  startRowOffset: number
  pointerX: number
  pointerY: number
  endIndex?: number
}

function sameIds(left: readonly string[], right: readonly string[]) {
  return left.length === right.length && left.every((id, index) => id === right[index])
}

function sameSelection(left: ReadonlySet<string>, right: ReadonlySet<string>) {
  return left.size === right.size && [...left].every(id => right.has(id))
}

/** Rebuild from the initial selection so reversing or shrinking restores every exited row. */
export function selectionForAccountRange(
  ids: readonly string[],
  initialSelection: ReadonlySet<string>,
  startIndex: number,
  endIndex: number,
): Set<string> {
  const next = new Set(initialSelection)
  if (
    !Number.isInteger(startIndex) || !Number.isInteger(endIndex)
    || startIndex < 0 || endIndex < 0 || startIndex >= ids.length || endIndex >= ids.length
  ) {
    return next
  }
  const selecting = !initialSelection.has(ids[startIndex])
  for (let index = Math.min(startIndex, endIndex); index <= Math.max(startIndex, endIndex); index++) {
    if (selecting)
      next.add(ids[index])
    else next.delete(ids[index])
  }
  return next
}

function dataRows(table: HTMLTableElement) {
  return Array.from(table.tBodies).flatMap(body =>
    Array.from(body.rows).filter(row => row.hasAttribute('data-row-key')),
  )
}

function hasModifier(event: MouseEvent) {
  return event.altKey || event.ctrlKey || event.metaKey || event.shiftKey
}

export function useAccountSwipeSelect(options: AccountSwipeSelectOptions) {
  const isDragging = shallowRef(false)
  const overlayStyle = shallowRef<CSSProperties>()
  let session: DragSession | undefined
  let frameId: number | undefined
  let disposed = false

  function endGesture() {
    const current = session
    session = undefined
    if (current) {
      if (frameId !== undefined)
        current.window.cancelAnimationFrame(frameId)
      const document = current.document
      document.removeEventListener('mousemove', onMouseMove, true)
      document.removeEventListener('mouseup', onMouseUp, true)
      document.removeEventListener('keydown', onKeyDown, true)
      document.removeEventListener('selectstart', preventSelection, true)
      document.removeEventListener('dragstart', preventSelection, true)
      document.removeEventListener('wheel', scheduleFrame, true)
      document.removeEventListener('scroll', scheduleFrame, true)
      current.window.removeEventListener('blur', endGesture)
      current.window.removeEventListener('resize', endGesture)
    }
    frameId = undefined
    isDragging.value = false
    overlayStyle.value = undefined
  }

  function ownsSelection(current: DragSession) {
    return toRaw(options.selectedIds.value) === current.selectionOwner
      && sameSelection(options.selectedIds.value, current.expectedSelection)
  }

  function readGeometry(current: DragSession) {
    const scroll = options.getScrollElement()
    const table = options.getTableElement()
    if (
      options.disabled.value || !sameIds(current.ids, options.rowIds.value) || !ownsSelection(current)
      || !scroll?.isConnected || !table?.isConnected || !scroll.contains(table)
      || table.ownerDocument !== current.document
    ) {
      return
    }

    // Row nodes can be replaced by a background refresh; only ordered IDs are retained.
    const rows = dataRows(table)
    if (!sameIds(current.ids, rows.map(row => row.getAttribute('data-row-key')!)))
      return
    const rowRects = rows.map(row => row.getBoundingClientRect())
    if (rowRects.some(rect => rect.height <= 0))
      return

    const scrollRect = scroll.getBoundingClientRect()
    const tableRect = table.getBoundingClientRect()
    const viewport = current.document.documentElement
    const contentTop = scrollRect.top + scroll.clientTop
    const contentLeft = scrollRect.left + scroll.clientLeft
    // BaseTable pins individual th cells, not the thead box itself.
    const headerBottom = Math.max(contentTop, ...Array.from(table.tHead?.rows ?? []).flatMap(row =>
      Array.from(row.cells).map(cell => cell.getBoundingClientRect().bottom),
    ))
    const clip = {
      top: Math.max(0, contentTop, tableRect.top, headerBottom),
      bottom: Math.min(viewport.clientHeight, scrollRect.bottom, contentTop + scroll.clientHeight, tableRect.bottom),
      left: Math.max(0, contentLeft, tableRect.left),
      right: Math.min(viewport.clientWidth, scrollRect.right, contentLeft + scroll.clientWidth, tableRect.right),
    }
    if (clip.top >= clip.bottom || clip.left >= clip.right)
      return
    return { scroll, rowRects, clip }
  }

  function updateGesture(autoscroll: boolean) {
    const current = session
    if (!current || !isDragging.value)
      return
    let geometry = readGeometry(current)
    if (!geometry) {
      endGesture()
      return
    }

    let continueScrolling = false
    if (autoscroll) {
      const { scroll, clip } = geometry
      const zone = Math.min(SCROLL_ZONE, (clip.bottom - clip.top) / 2)
      let step = 0
      if (current.pointerX >= clip.left && current.pointerX <= clip.right) {
        if (current.pointerY < clip.top + zone)
          step = -SCROLL_STEP
        else if (current.pointerY > clip.bottom - zone)
          step = SCROLL_STEP
      }
      const limit = Math.max(0, scroll.scrollHeight - scroll.clientHeight)
      const previous = scroll.scrollTop
      const next = Math.max(0, Math.min(limit, previous + step))
      if (step && next !== previous) {
        scroll.scrollTop = next
        const moved = scroll.scrollTop !== previous
        continueScrolling = moved && (step < 0 ? scroll.scrollTop > 0 : scroll.scrollTop < limit)
        geometry = readGeometry(current)
        if (!geometry) {
          endGesture()
          return
        }
      }
    }

    const { rowRects, clip } = geometry
    const pointerY = Math.max(clip.top, Math.min(clip.bottom, current.pointerY))
    let endIndex = -1
    let nearestDistance = Number.POSITIVE_INFINITY
    rowRects.forEach((rect, index) => {
      if (rect.bottom <= clip.top || rect.top >= clip.bottom)
        return
      const distance = Math.max(rect.top - pointerY, pointerY - rect.bottom, 0)
      if (distance < nearestDistance) {
        endIndex = index
        nearestDistance = distance
      }
    })
    if (endIndex < 0) {
      endGesture()
      return
    }

    if (current.endIndex !== endIndex) {
      const next = selectionForAccountRange(current.ids, current.initialSelection, current.startIndex, endIndex)
      current.endIndex = endIndex
      if (!sameSelection(next, options.selectedIds.value)) {
        // Keep a separate value snapshot to detect even reentrant in-place toolbar mutations.
        current.selectionOwner = next
        current.expectedSelection = new Set(next)
        options.selectedIds.value = next
        if (session !== current)
          return
      }
    }

    const startRect = rowRects[current.startIndex]!
    const anchorY = startRect.top + Math.min(current.startRowOffset, startRect.height)
    const top = Math.max(clip.top, Math.min(anchorY, pointerY))
    const bottom = Math.min(clip.bottom, Math.max(anchorY, pointerY))
    overlayStyle.value = bottom > top
      ? {
          position: 'fixed',
          pointerEvents: 'none',
          boxSizing: 'border-box',
          left: `${clip.left}px`,
          top: `${top}px`,
          width: `${clip.right - clip.left}px`,
          height: `${bottom - top}px`,
        }
      : undefined
    if (continueScrolling && session === current)
      scheduleFrame()
  }

  function scheduleFrame() {
    if (!session || !isDragging.value || frameId !== undefined)
      return
    frameId = session.window.requestAnimationFrame(() => {
      frameId = undefined
      updateGesture(true)
    })
  }

  function preventSelection(event: Event) {
    event.preventDefault()
  }

  function onMouseMove(event: MouseEvent) {
    const current = session
    if (!current)
      return
    if (event.buttons !== 1 || hasModifier(event)) {
      endGesture()
      return
    }
    current.pointerX = event.clientX
    current.pointerY = event.clientY
    if (!isDragging.value) {
      if (Math.abs(event.clientY - current.startY) < DRAG_THRESHOLD)
        return
      isDragging.value = true
      if (session !== current)
        return
      current.document.getSelection()?.removeAllRanges()
    }
    event.preventDefault()
    scheduleFrame()
  }

  function onMouseUp(event: MouseEvent) {
    if (event.button !== 0)
      return
    if (session && isDragging.value && !hasModifier(event)) {
      session.pointerX = event.clientX
      session.pointerY = event.clientY
      updateGesture(false)
    }
    endGesture()
  }

  function onKeyDown(event: KeyboardEvent) {
    if (event.key !== 'Escape' || !session)
      return
    const current = session
    const restore = ownsSelection(current)
    event.preventDefault()
    endGesture()
    if (restore && ownsSelection(current))
      options.selectedIds.value = new Set(current.initialSelection)
  }

  function onMouseDown(event: MouseEvent) {
    if (disposed)
      return
    endGesture()
    if (options.disabled.value || event.defaultPrevented || event.button !== 0 || event.buttons !== 1 || hasModifier(event))
      return
    if ('pointerType' in event && event.pointerType !== 'mouse')
      return
    if ('sourceCapabilities' in event && (event.sourceCapabilities as { firesTouchEvents?: boolean } | null)?.firesTouchEvents)
      return
    const target = event.target
    const table = options.getTableElement()
    const scroll = options.getScrollElement()
    if (!(target instanceof Element) || !table || !scroll || target.closest('table') !== table)
      return
    const cell = target.closest('td[data-column-key="identity"]')
    const row = cell?.parentElement
    if (
      !cell || !row || row.parentElement?.tagName !== 'TBODY' || row.parentElement.parentElement !== table
      || !row.hasAttribute('data-row-key') || target.closest(ignoredSelector)
    ) {
      return
    }
    const handle = target.closest('[data-swipe-select-handle]')
    if (
      !(handle && cell.contains(handle))
      && Array.from(target.childNodes).some(node => node.nodeType === 3 && node.textContent?.trim())
    ) {
      return
    }

    const ids = [...options.rowIds.value]
    const startIndex = ids.indexOf(row.getAttribute('data-row-key')!)
    const document = table.ownerDocument
    const window = document.defaultView
    if (startIndex < 0 || new Set(ids).size !== ids.length || !window)
      return
    const initialSelection = new Set(options.selectedIds.value)
    const current: DragSession = {
      document,
      window,
      ids,
      initialSelection,
      expectedSelection: initialSelection,
      selectionOwner: toRaw(options.selectedIds.value),
      startIndex,
      startY: event.clientY,
      startRowOffset: event.clientY - row.getBoundingClientRect().top,
      pointerX: event.clientX,
      pointerY: event.clientY,
    }
    const geometry = readGeometry(current)
    if (
      !geometry || event.clientX < geometry.clip.left || event.clientX > geometry.clip.right
      || event.clientY < geometry.clip.top || event.clientY > geometry.clip.bottom
    ) {
      return
    }
    session = current
    document.addEventListener('mousemove', onMouseMove, true)
    document.addEventListener('mouseup', onMouseUp, true)
    document.addEventListener('keydown', onKeyDown, true)
    document.addEventListener('selectstart', preventSelection, true)
    document.addEventListener('dragstart', preventSelection, true)
    document.addEventListener('wheel', scheduleFrame, { capture: true, passive: true })
    document.addEventListener('scroll', scheduleFrame, { capture: true, passive: true })
    window.addEventListener('blur', endGesture)
    window.addEventListener('resize', endGesture)
  }

  watch(() => [...options.rowIds.value], (ids) => {
    if (session && !sameIds(ids, session.ids))
      endGesture()
  }, { flush: 'sync' })
  watch(options.selectedIds, () => {
    if (session && !ownsSelection(session))
      endGesture()
  }, { flush: 'sync', deep: true })
  watch(options.disabled, endGesture, { flush: 'sync' })
  if (options.invalidationKey)
    watch(options.invalidationKey, endGesture, { flush: 'sync', deep: true })
  onScopeDispose(() => {
    disposed = true
    endGesture()
  })

  return { onMouseDown, isDragging, overlayStyle }
}
