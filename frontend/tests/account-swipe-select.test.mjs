/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'

const require = createRequire(import.meta.url)

// Deterministic DOM/event geometry without an added browser dependency. Integration
// checks still need the real BaseTable, sticky cells and browser layout engine.
class EventSurface {
  listeners = new Map()

  addEventListener(type, callback, options) {
    const capture = typeof options === 'boolean' ? options : !!options?.capture
    const key = `${type}:${capture}`
    if (!this.listeners.has(key))
      this.listeners.set(key, new Set())
    this.listeners.get(key).add(callback)
  }

  removeEventListener(type, callback, options) {
    const capture = typeof options === 'boolean' ? options : !!options?.capture
    this.listeners.get(`${type}:${capture}`)?.delete(callback)
  }

  dispatch(type, values = {}) {
    const event = Object.assign(new Event(type, { cancelable: true }), {
      button: 0,
      buttons: 1,
      clientX: 100,
      clientY: 240,
      ...values,
    })
    for (const capture of [true, false]) {
      for (const callback of [...(this.listeners.get(`${type}:${capture}`) ?? [])])
        callback(event)
    }
    return event
  }

  get listenerCount() {
    return [...this.listeners.values()].reduce((sum, listeners) => sum + listeners.size, 0)
  }
}

class ElementFixture {
  childNodes = []
  parentElement = null
  attributes = new Map()
  clientTop = 0
  clientLeft = 0

  constructor(tagName, attributes = {}) {
    this.tagName = tagName.toUpperCase()
    for (const [key, value] of Object.entries(attributes))
      this.attributes.set(key, value)
  }

  append(child) {
    child.parentElement = this
    child.ownerDocument = this.ownerDocument
    this.childNodes.push(child)
    return child
  }

  get children() {
    return this.childNodes.filter(node => node instanceof ElementFixture)
  }

  get isConnected() {
    return this.connectedRoot || this.parentElement?.isConnected || false
  }

  get tBodies() {
    return this.children.filter(node => node.tagName === 'TBODY')
  }

  get tHead() {
    return this.children.find(node => node.tagName === 'THEAD') ?? null
  }

  get rows() {
    return this.children.filter(node => node.tagName === 'TR')
  }

  get cells() {
    return this.children.filter(node => ['TH', 'TD'].includes(node.tagName))
  }

  hasAttribute(name) {
    return this.attributes.has(name)
  }

  getAttribute(name) {
    return this.attributes.get(name) ?? null
  }

  contains(element) {
    return element === this || (!!element.parentElement && this.contains(element.parentElement))
  }

  matches(selector) {
    const negation = selector.match(/:not\((.+)\)$/)
    if (negation)
      return this.matches(selector.slice(0, negation.index)) && !this.matches(negation[1])
    const tag = selector.match(/^[a-z]+/i)?.[0]
    if (tag && tag.toUpperCase() !== this.tagName)
      return false
    return [...selector.matchAll(/\[([^\]=]+)(?:="([^"]*)")?\]/g)]
      .every(([, key, value]) => this.hasAttribute(key) && (value === undefined || this.getAttribute(key) === value))
  }

  closest(selector) {
    if (selector.split(',').some(part => this.matches(part.trim())))
      return this
    return this.parentElement?.closest(selector) ?? null
  }

  getBoundingClientRect() {
    assert.ok(this.isConnected, 'the gesture must not read a detached row from a refresh')
    return this.measure()
  }
}

const filename = new URL('../src/views/accounts/composables/useAccountSwipeSelect.ts', import.meta.url)
const { outputText } = ts.transpileModule(readFileSync(filename, 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
})
const exports = {}
runInNewContext(outputText, {
  exports,
  require: name => name === 'vue' ? vue : require(name),
  Element: ElementFixture,
}, { filename: filename.pathname })
const { useAccountSwipeSelect, selectionForAccountRange } = exports

function assertSelection(actual, expected) {
  assert.deepEqual([...actual].sort(), [...expected].sort())
}

function rect(left, top, width, height) {
  return { left, top, right: left + width, bottom: top + height, width, height }
}

function fixture(t, { selected = [], ids = [...'abcdefghij'], layout: overrides = {} } = {}) {
  const layout = { left: 40, top: 100, width: 720, height: 360, tableWidth: 720, rowHeight: 40, ...overrides }
  const sourceIds = vue.ref([...ids])
  const document = new EventSurface()
  const window = new EventSurface()
  document.defaultView = window
  document.documentElement = { clientWidth: 800, clientHeight: 600 }
  let clearedTextSelections = 0
  document.getSelection = () => ({ removeAllRanges: () => clearedTextSelections++ })
  const frames = new Map()
  let nextFrame = 1
  window.requestAnimationFrame = (callback) => {
    const id = nextFrame++
    frames.set(id, callback)
    return id
  }
  window.cancelAnimationFrame = id => frames.delete(id)
  const scroll = new ElementFixture('div')
  scroll.connectedRoot = true
  scroll.ownerDocument = document
  scroll.measure = () => rect(layout.left, layout.top, layout.width, layout.height)
  scroll.scrollLeft = 0
  Object.defineProperties(scroll, {
    clientWidth: { get: () => layout.width },
    clientHeight: { get: () => layout.height },
    scrollHeight: { get: () => 40 + sourceIds.value.length * layout.rowHeight },
  })
  let scrollTop = 0
  let scrollWrites = 0
  Object.defineProperty(scroll, 'scrollTop', {
    get: () => scrollTop,
    set: (value) => {
      scrollWrites++
      scrollTop = Math.max(0, Math.min(value, scroll.scrollHeight - scroll.clientHeight))
    },
  })
  const table = scroll.append(new ElementFixture('table'))
  table.measure = () => rect(layout.left - scroll.scrollLeft, layout.top - scrollTop, layout.tableWidth, scroll.scrollHeight)
  const head = table.append(new ElementFixture('thead'))
  const headerCell = head.append(new ElementFixture('tr')).append(new ElementFixture('th', { 'data-column-key': 'identity' }))
  head.measure = () => rect(layout.left, layout.top - scrollTop, layout.tableWidth, 40)
  headerCell.measure = () => rect(layout.left, layout.top, layout.tableWidth, 40)
  const body = table.append(new ElementFixture('tbody'))
  const selectedIds = vue.ref(new Set(selected))
  const disabled = vue.ref(false)
  const invalidation = vue.ref({ page: 1, filter: '', sort: '', expanded: [], columns: ['identity'] })
  const options = {
    getScrollElement: () => scroll,
    getTableElement: () => table,
    rowIds: vue.computed(() => [...sourceIds.value]),
    selectedIds,
    disabled,
    invalidationKey: vue.computed(() => invalidation.value),
  }
  let rows
  function renderRows() {
    for (const child of body.children)
      child.parentElement = null
    body.childNodes = []
    rows = sourceIds.value.map((id, index) => {
      const row = body.append(new ElementFixture('tr', { 'data-row-key': id }))
      row.measure = () => rect(layout.left, layout.top + 40 + index * layout.rowHeight - scrollTop, layout.tableWidth, layout.rowHeight)
      const cell = row.append(new ElementFixture('td', { 'data-column-key': 'identity' }))
      const blank = cell.append(new ElementFixture('div'))
      const handle = blank.append(new ElementFixture('span', { 'data-swipe-select-handle': '' }))
      const icon = handle.append(new ElementFixture('svg')).append(new ElementFixture('path'))
      const text = blank.append(new ElementFixture('div', { 'data-swipe-select-ignore': '' }))
      text.append(new ElementFixture('span')).childNodes.push({ nodeType: 3, textContent: `Account ${id}` })
      return { row, cell, blank, handle, icon, text }
    })
  }
  renderRows()
  const scope = vue.effectScope()
  const drag = scope.run(() => useAccountSwipeSelect(options))
  t.after(() => {
    scope.stop()
    assert.equal(document.listenerCount + window.listenerCount, 0)
    assert.equal(frames.size, 0)
  })

  function down(index = 2, target = rows[index].cell, values = {}) {
    const event = Object.assign(new Event('mousedown', { cancelable: true }), {
      button: 0,
      buttons: 1,
      clientX: layout.left + 60,
      clientY: rows[index].row.getBoundingClientRect().top + layout.rowHeight / 2,
      ...values,
    })
    Object.defineProperty(event, 'target', { value: target })
    drag.onMouseDown(event)
    return event
  }
  function frame() {
    const pending = [...frames.values()]
    frames.clear()
    for (const callback of pending)
      callback()
  }
  function drainFrames() {
    let iterations = 0
    while (frames.size) {
      assert.ok(iterations++ < 500, 'autoscroll must stop scheduling at the scroll limit')
      frame()
    }
  }
  function assertReleased() {
    assert.equal(drag.isDragging.value, false)
    assert.equal(drag.overlayStyle.value, undefined)
    assert.equal(frames.size, 0)
    assert.equal(document.listenerCount + window.listenerCount, 0)
  }

  return {
    ...drag,
    selectedIds,
    sourceIds,
    disabled,
    invalidation,
    options,
    scope,
    document,
    window,
    scroll,
    table,
    body,
    headerCell,
    frames,
    layout,
    get rows() { return rows },
    get clearedTextSelections() { return clearedTextSelections },
    get scrollWrites() { return scrollWrites },
    down,
    frame,
    drainFrames,
    renderRows,
    assertReleased,
    move: (clientY, values) => document.dispatch('mousemove', { clientY, ...values }),
    up: (clientY = 240) => document.dispatch('mouseup', { clientY, buttons: 0 }),
  }
}

test('range selection is contiguous in either direction and preserves selections on other pages', () => {
  const initial = new Set(['off-page', 'b', 'e'])
  const ids = [...'abcde']
  assertSelection(selectionForAccountRange(ids, initial, 2, 4), ['off-page', 'b', 'c', 'd', 'e'])
  assertSelection(selectionForAccountRange(ids, initial, 2, 0), ['off-page', 'a', 'b', 'c', 'e'])
  assertSelection(selectionForAccountRange(ids, initial, 2, 2), ['off-page', 'b', 'c', 'e'])
  assertSelection(initial, ['off-page', 'b', 'e'])
  assert.deepEqual(ids, [...'abcde'])
})

test('selected start rows deselect; shrinking and reversing restore their initial state', () => {
  const initial = new Set(['off-page', 'a', 'c', 'e'])
  const ids = [...'abcde']
  assertSelection(selectionForAccountRange(ids, initial, 2, 4), ['off-page', 'a'])
  assertSelection(selectionForAccountRange(ids, initial, 2, 2), ['off-page', 'a', 'e'])
  assertSelection(selectionForAccountRange(ids, initial, 2, 0), ['off-page', 'e'])
  for (const [start, end] of [[-1, 2], [2, -1], [5, 1], [0, 5], [0.5, 1], [0, Number.NaN]]) {
    assertSelection(selectionForAccountRange(ids, initial, start, end), initial)
  }
  assertSelection(selectionForAccountRange([], initial, 0, 0), initial)
})

test('only identity icons and blank space arm a mouse gesture; text and controls stay native', (t) => {
  const f = fixture(t)
  for (const target of [f.rows[2].cell, f.rows[2].blank, f.rows[2].icon]) {
    assert.equal(f.down(2, target).defaultPrevented, false, 'mousedown must not become a click toggle')
    assert.ok(f.document.listenerCount > 0)
    assert.equal(f.document.dispatch('selectstart').defaultPrevented, true)
    f.up()
    f.assertReleased()
  }
  const identity = f.rows[2]
  const otherCell = identity.row.append(new ElementFixture('td', { 'data-column-key': 'health' }))
  const expansion = f.body.append(new ElementFixture('tr')).append(new ElementFixture('td', { 'data-column-key': 'identity' }))
  const outside = f.scroll.append(new ElementFixture('div'))
  const nested = identity.cell.append(new ElementFixture('table'))
    .append(new ElementFixture('tbody'))
    .append(new ElementFixture('tr', { 'data-row-key': 'c' }))
    .append(new ElementFixture('td', { 'data-column-key': 'identity' }))
  const unmarkedText = identity.cell.append(new ElementFixture('span'))
  unmarkedText.childNodes.push({ nodeType: 3, textContent: 'copy this name' })
  const forbidden = [identity.text, identity.text.children[0], otherCell, expansion, outside, nested, f.headerCell, unmarkedText]
  for (const tag of ['button', 'input', 'select', 'textarea', 'a', 'label', 'summary']) {
    forbidden.push(identity.handle.append(new ElementFixture(tag)).append(new ElementFixture('span')))
  }
  for (const attributes of [{ contenteditable: '' }, { contenteditable: 'plaintext-only' }, { role: 'button' }, { role: 'checkbox' }, { draggable: 'true' }]) {
    forbidden.push(identity.cell.append(new ElementFixture('div', attributes)).append(new ElementFixture('span')))
  }
  for (const target of forbidden) {
    f.down(2, target)
    f.assertReleased()
    assert.equal(f.document.dispatch('selectstart').defaultPrevented, false)
  }
  for (const values of [{ button: 1 }, { button: 2 }, { buttons: 3 }, { altKey: true }, { ctrlKey: true }, { metaKey: true }, { shiftKey: true }, { pointerType: 'touch' }, { pointerType: 'pen' }, { sourceCapabilities: { firesTouchEvents: true } }]) {
    f.down(2, undefined, values)
    f.assertReleased()
  }
  f.disabled.value = true
  f.down()
  f.assertReleased()
  assertSelection(f.selectedIds.value, [])
})

test('threshold is vertical and 5px; mouseup flushes batched movement without toggling clicks', (t) => {
  const f = fixture(t, { selected: ['off-page'] })
  f.down()
  f.move(244, { clientX: 700 })
  assert.equal(f.isDragging.value, false)
  assert.equal(f.frames.size, 0)
  f.up(244)
  assertSelection(f.selectedIds.value, ['off-page'])
  f.assertReleased()
  f.down()
  f.move(245)
  assert.equal(f.isDragging.value, true)
  assert.equal(f.clearedTextSelections, 1)
  f.move(280)
  f.move(320)
  assert.equal(f.frames.size, 1)
  assertSelection(f.selectedIds.value, ['off-page'])
  f.up(320)
  assertSelection(f.selectedIds.value, ['off-page', 'c', 'd', 'e'])
  f.assertReleased()
})

test('live reverse/shrink updates use the original selection and Escape restores the snapshot', (t) => {
  const f = fixture(t, { selected: ['off-page', 'c', 'e'] })
  f.down()
  f.move(320)
  f.frame()
  assertSelection(f.selectedIds.value, ['off-page'])
  f.move(245)
  f.frame()
  assertSelection(f.selectedIds.value, ['off-page', 'e'])
  f.move(200)
  f.frame()
  assertSelection(f.selectedIds.value, ['off-page', 'e'])
  assert.equal(f.document.dispatch('keydown', { key: 'Escape' }).defaultPrevented, true)
  assertSelection(f.selectedIds.value, ['off-page', 'c', 'e'])
  f.assertReleased()
})

test('scroll before activation keeps the mousedown row and mode, not the row now at that pixel', (t) => {
  const f = fixture(t, { selected: ['off-page', 'c', 'e'] })
  f.down()
  f.scroll.scrollTop = 80
  f.document.dispatch('scroll')
  assert.equal(f.isDragging.value, false)
  f.move(280)
  f.frame()
  assertSelection(f.selectedIds.value, ['off-page'])
  f.up(280)
  f.assertReleased()
})

test('same-ID refresh replaces DOM rows safely; wheel and scroll re-evaluate a stationary pointer', (t) => {
  const f = fixture(t)
  f.down()
  f.move(280)
  f.frame()
  assertSelection(f.selectedIds.value, ['c', 'd'])
  const oldRows = f.rows
  f.sourceIds.value = [...f.sourceIds.value]
  f.renderRows()
  assert.equal(oldRows[0].row.isConnected, false)
  assert.equal(f.isDragging.value, true)
  f.scroll.scrollTop = 40
  f.document.dispatch('wheel')
  f.frame()
  assertSelection(f.selectedIds.value, ['c', 'd', 'e'])
  f.scroll.scrollTop = 80
  f.document.dispatch('scroll')
  f.frame()
  assertSelection(f.selectedIds.value, ['c', 'd', 'e', 'f'])
  assert.equal(f.overlayStyle.value.top, '160px', 'the starting point moves with its live row')
  f.up(280)
})

test('the overlay uses visible table width and clips against sticky header cells and viewport', (t) => {
  const f = fixture(t, {
    ids: Array.from({ length: 30 }, (_, index) => String(index)),
    layout: { left: -40, width: 1000, height: 700, tableWidth: 1400 },
  })
  f.down()
  f.move(1000)
  f.frame()
  let style = f.overlayStyle.value
  assert.equal(style.position, 'fixed')
  assert.equal(style.pointerEvents, 'none')
  assert.equal(style.boxSizing, 'border-box')
  assert.equal(style.left, '0px')
  assert.equal(style.width, '800px')
  assert.equal(Number.parseFloat(style.top) + Number.parseFloat(style.height), 600)
  f.scroll.scrollTop = 40
  f.move(-50)
  f.frame()
  style = f.overlayStyle.value
  assert.equal(style.top, '140px', 'th stays pinned even though the thead box scrolls away')
  assert.ok(Number.parseFloat(style.height) >= 0)
  f.scroll.scrollTop = 200
  f.document.dispatch('scroll')
  f.frame()
  assert.equal(f.overlayStyle.value, undefined, 'an entirely clipped marquee must not escape above the header')
  f.document.dispatch('keydown', { key: 'Escape' })
  f.assertReleased()
})

test('60px edge autoscroll tracks stationary pointers and stops at both limits without paging', (t) => {
  const f = fixture(t)
  f.down()
  f.move(399)
  f.frame()
  assert.equal(f.scroll.scrollTop, 0, 'outside the 60px bottom zone')
  assert.equal(f.frames.size, 0)
  f.move(401)
  f.frame()
  assert.equal(f.scroll.scrollTop, 8)
  assert.equal(f.frames.size, 1)
  f.move(460)
  f.drainFrames()
  assert.equal(f.scroll.scrollTop, 80)
  assertSelection(f.selectedIds.value, [...'cdefghij'])
  assert.equal(f.invalidation.value.page, 1)
  assert.equal(f.frames.size, 0)
  f.move(140)
  f.drainFrames()
  assert.equal(f.scroll.scrollTop, 0)
  assertSelection(f.selectedIds.value, ['a', 'b', 'c'])
  assert.equal(f.frames.size, 0)
  const writes = f.scrollWrites
  f.move(460, { clientX: 900 })
  f.frame()
  assert.equal(f.scrollWrites, writes, 'leaving the visible horizontal bounds stops autoscroll')
  f.up(460)
})

test('pending and active gestures release frames and all listeners on blur, resize and scope disposal', (t) => {
  for (const active of [false, true]) {
    for (const reason of ['blur', 'resize', 'unmount', 'lost-button']) {
      const f = fixture(t, { selected: ['off-page'] })
      f.down()
      if (active) {
        f.move(320)
        f.frame()
        f.move(450)
        assert.equal(f.frames.size, 1)
      }
      const before = new Set(f.selectedIds.value)
      if (reason === 'unmount')
        f.scope.stop()
      else if (reason === 'lost-button')
        f.move(450, { buttons: 0 })
      else f.window.dispatch(reason)
      f.assertReleased()
      f.document.dispatch('keydown', { key: 'Escape' })
      f.frame()
      assertSelection(f.selectedIds.value, before)
    }
  }
})

test('ID order, invalidation and disabled changes end synchronously and never flush stale movement', (t) => {
  for (const active of [false, true]) {
    for (const change of [
      f => f.sourceIds.value.reverse(),
      f => f.sourceIds.value.splice(0, 1),
      f => f.invalidation.value.page++,
      f => f.invalidation.value.filter = 'updated',
      f => f.invalidation.value.sort = 'ascending',
      f => f.invalidation.value.expanded.push('c'),
      f => f.invalidation.value.columns.push('health'),
      f => f.disabled.value = true,
    ]) {
      const f = fixture(t)
      f.down()
      if (active) {
        f.move(280)
        f.frame()
        f.move(360)
      }
      const before = new Set(f.selectedIds.value)
      change(f)
      f.assertReleased()
      f.up(360)
      f.frame()
      assertSelection(f.selectedIds.value, before)
    }
  }
})

test('external selection replacement and Set mutation end both pending and active gestures', (t) => {
  for (const active of [false, true]) {
    for (const change of [
      f => f.selectedIds.value = new Set(['toolbar']),
      f => f.selectedIds.value.add('toolbar'),
      f => f.selectedIds.value.clear(),
      f => f.selectedIds.value = new Set(f.selectedIds.value),
    ]) {
      const f = fixture(t, { selected: ['off-page'] })
      f.down()
      if (active) {
        f.move(280)
        f.frame()
        f.move(360)
      }
      change(f)
      const before = new Set(f.selectedIds.value)
      f.assertReleased()
      f.up(360)
      f.document.dispatch('keydown', { key: 'Escape' })
      assertSelection(f.selectedIds.value, before)
    }
  }
})

test('a reentrant external Set mutation during publication cannot be mistaken for an owned write', (t) => {
  const f = fixture(t)
  const stop = vue.watch(f.selectedIds, () => {
    if (f.selectedIds.value.has('c'))
      f.selectedIds.value.add('toolbar')
  }, { flush: 'sync' })
  t.after(stop)
  f.down()
  f.move(280)
  f.frame()
  f.assertReleased()
  assertSelection(f.selectedIds.value, ['c', 'd', 'toolbar'])
  f.up(360)
  assertSelection(f.selectedIds.value, ['c', 'd', 'toolbar'])
})

test('Escape cleanup does not restore over a reentrant toolbar selection change', (t) => {
  const f = fixture(t)
  const stop = vue.watch(f.isDragging, (dragging) => {
    if (!dragging)
      f.selectedIds.value = new Set(['toolbar'])
  }, { flush: 'sync' })
  t.after(stop)
  f.down()
  f.move(280)
  f.frame()
  f.document.dispatch('keydown', { key: 'Escape' })
  f.assertReleased()
  assertSelection(f.selectedIds.value, ['toolbar'])
})

test('DOM row mismatch, hidden or detached tables terminate safely without retaining stale nodes', (t) => {
  for (const change of [
    f => f.body.childNodes.reverse(),
    f => f.body.childNodes.pop(),
    f => f.scroll.connectedRoot = false,
    f => f.layout.width = 0,
    f => f.options.getTableElement = () => undefined,
    f => f.options.getScrollElement = () => undefined,
  ]) {
    const f = fixture(t, { selected: ['off-page'] })
    f.down()
    f.move(320)
    change(f)
    f.frame()
    f.assertReleased()
    assertSelection(f.selectedIds.value, ['off-page'])
  }
  const duplicate = fixture(t, { ids: ['a', 'b', 'b'] })
  duplicate.down()
  duplicate.assertReleased()
})
