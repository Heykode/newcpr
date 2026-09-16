/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import { createRenderer, createSSRApp, defineComponent, h, nextTick, ref } from 'vue'
import { compileScript, parse } from 'vue/compiler-sfc'
import { renderToString } from 'vue/server-renderer'

const require = createRequire(import.meta.url)
const now = Date.parse('2026-09-13T02:00:00Z')
const clock = ref(new Date(now))
const modules = new Map()
const popover = defineComponent({
  setup: (_props, { slots }) => () => h('div', [
    slots.trigger?.({ open: false }),
    slots.default?.({ open: true }),
  ]),
})

function loadSource(filename) {
  if (modules.has(filename.href))
    return modules.get(filename.href)
  const exports = {}
  modules.set(filename.href, exports)
  const source = readFileSync(filename, 'utf8')
  const content = filename.pathname.endsWith('.vue')
    ? compileScript(parse(source, { filename: filename.pathname }).descriptor, {
      id: filename.pathname,
      inlineTemplate: true,
    }).content
    : source
  const { outputText } = ts.transpileModule(content, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  runInNewContext(outputText, {
    exports,
    require(name) {
      if (name === '@/composables/useUiClock')
        return { useUiClock: () => clock }
      if (name === '@/components/base/BasePopover.vue')
        return popover
      if (name.startsWith('@/') || name.startsWith('.')) {
        const path = name.endsWith('.vue') ? name : `${name}.ts`
        return loadSource(name.startsWith('@/')
          ? new URL(`../src/${path.slice(2)}`, import.meta.url)
          : new URL(path, filename))
      }
      return require(name)
    },
  }, { filename: filename.pathname })
  return exports
}

const components = '../src/views/accounts/components/'
const presenter = loadSource(new URL(`${components}AccountUsageWindow/presenter.ts`, import.meta.url))
const resetView = presenter.quotaWindowResetPresentation
const windowCode = presenter.quotaWindowCode

function quotaWindow(minutes = 130, overrides = {}) {
  return {
    key: 'primary',
    group: 'shortTerm',
    limitId: 'codex',
    limitName: 'Codex',
    role: 'primary',
    windowSeconds: 18_000,
    labelDisplay: 'Codex 5小时',
    windowLabelDisplay: '5小时',
    usedPercent: 100,
    usedPercentDisplay: '100%',
    limitReached: true,
    resetAt: new Date(now + minutes * 60_000).toISOString(),
    resetAtDisplay: new Date(now + (minutes + 480) * 60_000).toISOString().slice(0, 19).replace('T', ' '),
    ...overrides,
  }
}

async function renderComponent(path, props) {
  const component = loadSource(new URL(`${components}${path}`, import.meta.url)).default
  return renderToString(createSSRApp(component, props))
}

function element(tag, text = '') {
  return { tag, text, props: {}, children: [], parent: null }
}

function removeElement(node) {
  if (node.parent) {
    node.parent.children.splice(node.parent.children.indexOf(node), 1)
    node.parent = null
  }
}

const renderer = createRenderer({
  createElement: tag => element(tag),
  createText: text => element('#text', text),
  createComment: text => element('#comment', text),
  setText: (node, text) => { node.text = text },
  setElementText(node, text) {
    node.text = text
    node.children = []
  },
  patchProp: (node, key, _previous, value) => { node.props[key] = value },
  parentNode: node => node.parent,
  nextSibling: node => node.parent?.children[node.parent.children.indexOf(node) + 1] ?? null,
  insert(node, parent, anchor = null) {
    removeElement(node)
    const index = anchor ? parent.children.indexOf(anchor) : parent.children.length
    parent.children.splice(index, 0, node)
    node.parent = parent
  },
  remove: removeElement,
})

function mountComponent(context, path, initialProps) {
  const component = loadSource(new URL(`${components}${path}`, import.meta.url)).default
  const props = ref(initialProps)
  const root = element('root')
  const app = renderer.createApp({ render: () => h(component, props.value) })
  app.mount(root)
  context.after(() => {
    app.unmount()
    clock.value = new Date(now)
  })
  return { root, props }
}

function descendants(node, predicate) {
  return [
    ...(predicate(node) ? [node] : []),
    ...node.children.flatMap(child => descendants(child, predicate)),
  ]
}

function textContent(node) {
  if (node.tag === '#comment')
    return ''
  return node.text + node.children.map(textContent).join('')
}

function quotaAccount(windows, usage = {}) {
  return {
    authenticationKind: 'oauth',
    quota: { windows },
    usage: {
      requestCount: 4,
      totalTokensDisplay: '12.5K',
      windowLabelDisplay: '今日',
      costs: [{ currency: 'USD', estimatedAmount: '12.50', estimatedAmountDisplay: '$12.50' }],
      models: [],
      ...usage,
    },
  }
}

function assertSummaryRows(root, expected) {
  const triggers = descendants(root, node => node.tag === 'button' && node.props['aria-haspopup'] === 'dialog' && /^查看.*详情$/.test(node.props['aria-label'] ?? ''))
  assert.equal(triggers.length, 1)
  assert.notEqual(forecastButton(root), triggers[0], 'forecast and per-cycle details remain independent controls')
  const rows = descendants(triggers[0], node => node.props.role === 'group')
  assert.equal(rows.length, expected.length)
  assert.equal(descendants(triggers[0], node => node.props.role === 'progressbar').length, expected.length)
  assert.equal(descendants(triggers[0], node => node.tag === 'strong').length, expected.length)
  rows.forEach((row, index) => {
    const { window, code, countdown } = expected[index]
    const children = row.children.filter(node => !node.tag.startsWith('#'))
    assert.equal(row.props['aria-label'], `${window.labelDisplay}额度周期`)
    assert.match(row.props.class, /(?:^|\s)grid(?:\s|$)/)
    assert.deepEqual(children.map(node => node.tag), ['span', 'div', 'strong', countdown === null ? 'span' : 'time'])
    const [label, bar, percent, reset] = children
    assert.equal(textContent(label), code)
    assert.equal(label.props.title, window.labelDisplay)
    assert.equal(bar.props.role, 'progressbar')
    assert.equal(bar.props['aria-label'], window.labelDisplay)
    assert.equal(bar.props['aria-valuenow'], window.usedPercent ?? undefined)
    assert.equal(bar.props['aria-valuetext'], window.usedPercentDisplay)
    assert.equal(bar.children[0].props.style.width, `${window.usedPercent ?? 0}%`)
    assert.equal(textContent(percent), window.usedPercentDisplay)
    assert.equal(percent.props['aria-label'], `${window.labelDisplay}已使用${window.usedPercentDisplay}`)
    for (let node = percent; node; node = node.parent) {
      assert.ok(!node.props.hidden)
      assert.notEqual(String(node.props['aria-hidden']), 'true')
      assert.doesNotMatch(node.props.class ?? '', /(?:^|\s)(?:hidden|invisible|sr-only|opacity-0)(?:\s|$)/)
      assert.notEqual(node.props.style?.display, 'none')
      assert.notEqual(node.props.style?.visibility, 'hidden')
    }
    if (countdown === null) {
      assert.equal(textContent(reset), '—')
      assert.equal(reset.props.title, '未提供重置时间')
    }
    else {
      assert.equal(textContent(reset), countdown)
      assert.equal(reset.props.datetime, window.resetAt)
      assert.equal(reset.props['aria-label'], `${window.labelDisplay}：${countdown}${countdown === '待确认' ? '' : '后重置'}`)
    }
  })
  return rows
}

function forecastButton(root) {
  const buttons = descendants(root, node => node.tag === 'button' && textContent(node).trim().startsWith('预计周额度：'))
  assert.equal(buttons.length, 1)
  assert.equal(buttons[0].props.type, 'button')
  assert.equal(buttons[0].props['aria-haspopup'], 'dialog')
  assert.ok(!buttons[0].props.disabled)
  return buttons[0]
}

test('real summary recalculates current weekly costs immediately and expires the window', async (context) => {
  clock.value = new Date(now)
  const usage = {
    quotaWindow: {
      key: 'week',
      period: 'weekly',
      usedPercent: 1,
      estimatedUsd: 50,
      resetAt: new Date(now + 60_000).toISOString(),
    },
    costs: [{ currency: 'USD', estimatedAmount: '0.5' }],
  }
  const { root, props } = mountComponent(context, 'AccountQuotaSummaryCell/index.vue', {
    account: quotaAccount([quotaWindow()], usage),
  })
  assert.equal(textContent(forecastButton(root)).trim(), '预计周额度：$50.00')
  assert.doesNotMatch(textContent(forecastButton(root)), /≈/)
  props.value = { account: quotaAccount([quotaWindow()], { ...usage, quotaWindow: { ...usage.quotaWindow, estimatedUsd: 80 }, costs: [{ currency: 'USD', estimatedAmount: '0.8' }] }) }
  await nextTick()
  assert.equal(textContent(forecastButton(root)).trim(), '预计周额度：$80.00')
  props.value = { account: quotaAccount([quotaWindow()], { ...usage, quotaWindow: { ...usage.quotaWindow, usedPercent: 2, estimatedUsd: 25 } }) }
  await nextTick()
  assert.equal(textContent(forecastButton(root)).trim(), '预计周额度：$25.00')
  clock.value = new Date(now + 60_000)
  await nextTick()
  assert.equal(textContent(forecastButton(root)).trim(), '预计周额度：—')
  props.value = { account: quotaAccount([quotaWindow()], { ...usage, quotaWindow: null }) }
  await nextTick()
  assert.equal(textContent(forecastButton(root)).trim(), '预计周额度：—')
  clock.value = new Date(now)
})

function assertSummaryFooter(root, expectedCost) {
  const button = forecastButton(root)
  const footer = button.parent
  assert.equal(footer, root.children[0].children.at(-1), 'the forecast footer remains outside conditional quota content')
  const classes = footer.props.class.split(/\s+/)
  for (const name of ['flex', 'min-w-0', 'flex-wrap', 'items-baseline', 'gap-x-2', 'gap-y-1'])
    assert.ok(classes.includes(name), `footer retains ${name}`)
  assert.match(button.props.class, /(?:^|\s)min-w-0(?:\s|$)/)
  assert.match(button.props.class, /(?:^|\s)wrap-anywhere(?:\s|$)/)
  const children = footer.children.filter(node => !node.tag.startsWith('#'))
  assert.deepEqual(children.map(node => node.tag), expectedCost === null ? ['button'] : ['span', 'button'])
  assert.equal(children.at(-1), button, 'the forecast button follows the cost in the same footer')
  if (expectedCost !== null) {
    assert.equal(textContent(children[0]).trim(), `消费：${expectedCost}`)
    assert.match(children[0].props.class, /(?:^|\s)min-w-0(?:\s|$)/)
    assert.match(children[0].props.class, /(?:^|\s)break-all(?:\s|$)/)
    assert.equal(children[0].props.title, expectedCost === '—' ? '暂无统计数据，不代表账号不可用' : undefined)
  }
  else {
    assert.doesNotMatch(textContent(root), /消费：/)
  }
  assert.equal(footer.props.title, undefined, 'missing cost statistics must not label the forecast button')
  return button
}

test('quota countdown formats days, hours, minutes and the last partial minute without early expiry', () => {
  for (const [minutes, display] of [
    [6 * 1440 + 7 * 60, '6d 7h'],
    [6 * 1440 + 20 * 60 + 59, '6d 20h'],
    [1440, '1d 0h'],
    [1439, '23h 59m'],
    [130, '2h 10m'],
    [60, '1h 0m'],
    [59, '59m'],
    [25, '25m'],
    [1, '1m'],
    [0.5, '<1m'],
    [1 / 60_000, '<1m'],
  ]) {
    const view = resetView(quotaWindow(minutes), now)
    assert.equal(view.display, display)
    assert.equal(view.pending, false)
  }
})

test('expired resets stay pending even at zero usage and never change the source quota', () => {
  for (const minutes of [0, -1, -1440]) {
    for (const usedPercent of [0, 100, null]) {
      const window = Object.freeze(quotaWindow(minutes, { usedPercent }))
      const snapshot = JSON.stringify(window)
      const view = resetView(window, now)
      assert.equal(view.display, '待确认')
      assert.equal(view.pending, true)
      assert.match(view.title, /等待上游确认额度恢复/)
      assert.equal(JSON.stringify(window), snapshot)
    }
  }
})

test('old responses and missing, invalid or timezone-free timestamps never derive a countdown from display text', () => {
  for (const resetAt of [undefined, null, '', 'invalid', '2026-09-13 12:10:00', '2026-09-13T12:10:00', '2026-99-13T12:10:00Z', '2026-09-13T12:10:00+99:00']) {
    assert.equal(resetView(quotaWindow(130, { resetAt }), now), null)
  }
  assert.equal(resetView(undefined, now), null)
  assert.equal(resetView(quotaWindow(), Number.NaN), null)
})

test('timezone-qualified instants match across UTC offsets, including daylight-saving offsets', () => {
  for (const resetAt of ['2026-09-13T04:10:00Z', '2026-09-13T12:10:00+08:00', '2026-09-13T00:10:00-04:00']) {
    assert.equal(resetView(quotaWindow(130, { resetAt }), now).display, '2h 10m')
  }
  assert.match(resetView(quotaWindow(), now).title, /12:10:00 \(UTC\+8\)/)
  assert.match(resetView(quotaWindow(130, { resetAtDisplay: '—' }), now).title, /2026-09-13T04:10:00\.000Z/)
})

test('period labels use reported durations and retain safe fallbacks for unusual windows', () => {
  assert.equal(windowCode(18_000, 'primary'), '5H')
  assert.equal(windowCode(604_800, 'secondary'), '7D')
  assert.equal(windowCode(259_200, 'secondary'), '3D')
  for (const seconds of [null, -1, 0, Number.NaN, Number.POSITIVE_INFINITY]) {
    assert.equal(windowCode(seconds, 'primary'), 'P')
    assert.equal(windowCode(seconds, 'secondary'), 'S')
    assert.equal(windowCode(seconds, 'monthly'), 'M')
    assert.equal(windowCode(seconds, null), 'Q')
  }
})

test('real summary component pairs each countdown with its own period and unchanged progress bar', async () => {
  clock.value = new Date(now)
  const html = await renderComponent('AccountQuotaSummaryCell/WindowGroup.vue', {
    label: 'Codex',
    windows: [
      quotaWindow(),
      quotaWindow(6 * 1440 + 20 * 60, { key: 'secondary', windowSeconds: 604_800, role: 'secondary', labelDisplay: 'Codex 7天', usedPercent: 35, usedPercentDisplay: '35%' }),
    ],
  })
  const times = [...html.matchAll(/<time\b[^>]*>[^<]*<\/time>/g)].map(match => match[0])
  assert.equal(times.length, 2)
  assert.match(times[0], /Codex 5小时：2h 10m后重置/)
  assert.match(times[1], /Codex 7天：6d 20h后重置/)
  assert.match(html, />5H<\/span>/)
  assert.match(html, />7D<\/span>/)
  assert.match(html, /aria-label="Codex 5小时"[^>]*aria-valuenow="100"/)
  assert.match(html, /aria-label="Codex 7天"[^>]*aria-valuenow="35"/)
})

test('one shared clock updates mounted summary and detail surfaces without remounting or resetting quota', async (context) => {
  clock.value = new Date(now)
  const window = Object.freeze(quotaWindow(1))
  const windows = Object.freeze([window])
  const snapshot = JSON.stringify(windows)
  const surfaces = [
    'AccountQuotaSummaryCell/WindowGroup.vue',
    'AccountQuotaSummaryCell/Entry.vue',
    'AccountQuotaPanel/Entry.vue',
    'AccountQuotaPanel/UsageLimits.vue',
    'AccountUsageWindow/index.vue',
  ].map(path => ({
    path,
    ...mountComponent(context, path, { label: 'Codex', windows, window }),
  }))

  for (const [elapsed, expected] of [
    [0, '1m'],
    [30_000, '<1m'],
    [60_000, '待确认'],
    [2 * 24 * 60 * 60_000, '待确认'],
  ]) {
    clock.value = new Date(now + elapsed)
    await nextTick()
    for (const { path, root } of surfaces) {
      assert.ok(textContent(root).includes(expected), `${path}: ${textContent(root)}`)
      const bars = descendants(root, node => node.props.role === 'progressbar')
      assert.ok(bars.length > 0, path)
      for (const bar of bars) {
        const expectedPercent = path.endsWith('UsageLimits.vue') ? 0 : 100
        assert.equal(bar.props['aria-valuenow'], expectedPercent, path)
        assert.equal(bar.children[0].props.style.width, `${expectedPercent}%`, path)
      }
      if (elapsed >= 60_000) {
        assert.doesNotMatch(textContent(root), /后重置|待确认后|已恢复/, path)
        assert.ok(descendants(root, node => node.props.title?.includes('等待上游确认额度恢复')).length > 0, path)
      }
    }
    assert.equal(JSON.stringify(windows), snapshot)
  }
})

test('summary omits countdown for legacy responses, and local windows keep rolling request statistics', async () => {
  clock.value = new Date(now)
  const legacy = await renderComponent('AccountQuotaSummaryCell/WindowGroup.vue', {
    label: 'Codex',
    windows: [quotaWindow(130, { resetAt: undefined })],
  })
  assert.doesNotMatch(legacy, /<time\b/)
  assert.match(legacy, /未提供重置时间/)
  const local = await renderComponent('AccountQuotaSummaryCell/Entry.vue', {
    label: null,
    windows: [quotaWindow(130, { usedPercent: null, localUsage: { requestCount: 3 } })],
  })
  assert.match(local, /滚动窗口/)
  assert.doesNotMatch(local, /后重置|待确认|重置时间/)
})

test('popover and detail surfaces retain absolute reset times alongside the countdown', async () => {
  clock.value = new Date(now)
  for (const path of [
    'AccountQuotaSummaryCell/Entry.vue',
    'AccountQuotaPanel/Entry.vue',
    'AccountQuotaPanel/UsageLimits.vue',
    'AccountUsageWindow/index.vue',
  ]) {
    const window = quotaWindow()
    const html = await renderComponent(path, { label: 'Codex', windows: [window], window })
    assert.match(html, /2h 10m/, path)
    assert.match(html, /2026-09-13 12:10:00/, path)
    const expired = quotaWindow(-1)
    const pending = await renderComponent(path, { label: 'Codex', windows: [expired], window: expired })
    assert.match(pending, /待确认/, path)
    assert.doesNotMatch(pending, /已恢复|可用账号/, path)
  }
})

test('raw timestamps alone determine countdowns even when display text is stale or unparseable', () => {
  for (const resetAtDisplay of ['2000-01-01 00:00:00', 'not a date', '—', '']) {
    const window = quotaWindow(130, { resetAtDisplay })
    assert.equal(resetView(window, now).display, '2h 10m')
    assert.equal(resetView(window, now).pending, false)
  }
  for (const resetAt of [0, false, {}, [], undefined, null]) {
    assert.equal(resetView(quotaWindow(130, { resetAt }), now), null)
  }
  for (const invalidNow of [Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY]) {
    assert.equal(resetView(quotaWindow(), invalidNow), null)
  }
})

test('API nanosecond precision and millisecond expiry boundaries remain compatible', () => {
  const window = quotaWindow(1, { resetAt: '2026-09-13T02:01:00.123456789Z' })
  assert.equal(resetView(window, now + 60_122).display, '<1m')
  assert.equal(resetView(window, now + 60_123).display, '待确认')
  const offset = quotaWindow(1, { resetAt: '2026-09-13T10:01:00.123456789+08:00' })
  assert.equal(resetView(offset, now + 60_122).display, '<1m')
})

test('missing reset timestamps preserve absolute text and never invent pending state on any detail surface', async () => {
  clock.value = new Date(now + 365 * 24 * 60 * 60_000)
  try {
    for (const resetAt of [undefined, null, 'invalid']) {
      const window = quotaWindow(130, { resetAt })
      for (const path of [
        'AccountQuotaSummaryCell/Entry.vue',
        'AccountQuotaPanel/Entry.vue',
        'AccountQuotaPanel/UsageLimits.vue',
        'AccountUsageWindow/index.vue',
      ]) {
        const html = await renderComponent(path, { label: 'Codex', windows: [window], window })
        assert.match(html, /2026-09-13 12:10:00/, path)
        assert.doesNotMatch(html, /<time\b|待确认|后重置|2h 10m/, path)
      }
    }
  }
  finally {
    clock.value = new Date(now)
  }
})

test('unknown and local windows without reset timestamps never derive reset times from their duration', async () => {
  clock.value = new Date(now)
  for (const localUsage of [undefined, { requestCount: 0 }, { requestCount: 3 }]) {
    const window = quotaWindow(130, {
      usedPercent: null,
      usedPercentDisplay: '—',
      limitReached: false,
      limitId: null,
      resetAt: null,
      resetAtDisplay: '—',
      localUsage,
    })
    for (const path of [
      'AccountQuotaSummaryCell/Entry.vue',
      'AccountQuotaPanel/Entry.vue',
      'AccountQuotaPanel/UsageLimits.vue',
      'AccountUsageWindow/index.vue',
    ]) {
      const html = await renderComponent(path, { label: null, windows: [window], window })
      assert.doesNotMatch(html, /<time\b|待确认|后重置|已恢复|2026-/, path)
      assert.doesNotMatch(html, /aria-valuenow="0"|aria-valuenow="100"/, path)
    }
  }
  const empty = await renderComponent('AccountUsageWindow/index.vue', {})
  assert.match(empty, /额度待观测/)
  assert.doesNotMatch(empty, /progressbar|重置|待确认/)
})

test('real summary grouping keeps timestamps attached after duration sorting and keyed API updates', async (context) => {
  clock.value = new Date(now)
  const short = quotaWindow(15)
  const weekly = quotaWindow(6 * 1440, {
    key: 'weekly',
    role: 'secondary',
    windowSeconds: 604_800,
    labelDisplay: 'Codex 7天',
    windowLabelDisplay: '7天',
    usedPercent: 25,
    usedPercentDisplay: '25%',
  })
  const monthly = quotaWindow(30 * 1440, {
    key: 'monthly',
    role: 'monthly',
    group: 'monthly',
    windowSeconds: 2_592_000,
    resetAt: null,
    resetAtDisplay: '—',
    labelDisplay: 'Codex 30天',
    windowLabelDisplay: '30天',
  })
  const account = quotaAccount([monthly, weekly, short], { requestCount: 0, costs: [] })
  const { root, props } = mountComponent(context, 'AccountQuotaSummaryCell/index.vue', { account })
  const initialRows = assertSummaryRows(root, [
    { window: short, code: '5H', countdown: '15m' },
    { window: weekly, code: '7D', countdown: '6d 0h' },
    { window: monthly, code: '30D', countdown: null },
  ])
  assert.equal(account.quota.windows[0], monthly, 'sorting must not mutate API window order')

  const updatedShort = quotaWindow(60, { usedPercent: 10, usedPercentDisplay: '10%', limitReached: false })
  const updatedWeekly = { ...weekly, usedPercent: null, usedPercentDisplay: '—', resetAt: null, resetAtDisplay: '—' }
  const updatedMonthly = { ...monthly, resetAt: quotaWindow(25).resetAt, usedPercent: 80, usedPercentDisplay: '80%' }
  props.value = {
    account: { ...account, quota: { windows: [updatedWeekly, updatedMonthly, updatedShort] } },
  }
  await nextTick()
  const updatedRows = assertSummaryRows(root, [
    { window: updatedShort, code: '5H', countdown: '1h 0m' },
    { window: updatedWeekly, code: '7D', countdown: null },
    { window: updatedMonthly, code: '30D', countdown: '25m' },
  ])
  updatedRows.forEach((row, index) => assert.equal(row, initialRows[index]))
  const articles = descendants(root, node => node.tag === 'article')
  const weeklyDetail = articles.find(node => textContent(node).includes('7天'))
  assert.ok(weeklyDetail)
  assert.doesNotMatch(textContent(weeklyDetail), /6d 0h|周期重置|待确认/)

  clock.value = new Date(now + 25 * 60_000)
  await nextTick()
  assertSummaryRows(root, [
    { window: updatedShort, code: '5H', countdown: '35m' },
    { window: updatedWeekly, code: '7D', countdown: null },
    { window: updatedMonthly, code: '30D', countdown: '待确认' },
  ])

  const daily = quotaWindow(2 * 1440, {
    key: 'daily',
    role: 'secondary',
    windowSeconds: 86_400,
    labelDisplay: 'Codex 1天',
    windowLabelDisplay: '1天',
    usedPercent: 0,
    usedPercentDisplay: '0%',
    limitReached: false,
  })
  props.value = {
    account: { ...account, quota: { windows: [updatedMonthly, daily, updatedWeekly] } },
  }
  await nextTick()
  const finalRows = assertSummaryRows(root, [
    { window: daily, code: '1D', countdown: '1d 23h' },
    { window: updatedWeekly, code: '7D', countdown: null },
    { window: updatedMonthly, code: '30D', countdown: '待确认' },
  ])
  assert.notEqual(finalRows[0], initialRows[0])
  assert.equal(finalRows[1], initialRows[1])
  assert.equal(finalRows[2], initialRows[2])
})

test('remaining quota in usage limits stays unchanged when only one of multiple cycles expires', async (context) => {
  clock.value = new Date(now)
  const windows = [
    quotaWindow(1),
    quotaWindow(24 * 60, {
      key: 'weekly',
      role: 'secondary',
      labelDisplay: 'Codex 7天',
      windowSeconds: 604_800,
      usedPercent: 25,
      usedPercentDisplay: '25%',
      limitReached: false,
    }),
  ]
  const { root } = mountComponent(context, 'AccountQuotaPanel/UsageLimits.vue', { windows })
  clock.value = new Date(now + 60_000)
  await nextTick()
  const bars = descendants(root, node => node.props.role === 'progressbar')
  assert.deepEqual(bars.map(node => node.props['aria-valuenow']), [0, 75])
  assert.equal(descendants(root, node => node.props.title?.includes('等待上游确认额度恢复')).length, 1)
  assert.ok(textContent(root).includes('23h 59m后重置'))
})

test('summary exposes real reset timestamps even when every utilization is unknown', async () => {
  clock.value = new Date(now)
  const window = quotaWindow(130, { usedPercent: null, usedPercentDisplay: '—', limitReached: false })
  const html = await renderComponent('AccountQuotaSummaryCell/Entry.vue', { label: 'Codex', windows: [window] })
  const trigger = html.match(/<button\b[^>]*>[\s\S]*?<\/button>/)?.[0]
  assert.ok(trigger)
  assert.match(trigger, /<time\b[^>]*>2h 10m<\/time>/)
  assert.doesNotMatch(trigger, /aria-valuenow="0"|aria-valuenow="100"/)
})

for (const [highest, shortPercent, weeklyPercent] of [
  ['5H', 86, 35],
  ['7D', 23, 91],
]) {
  test(`real SummaryCell keeps both visible cycle percentages when ${highest} is highest without a mixed header rate`, async (context) => {
    const short = quotaWindow(130, { usedPercent: shortPercent, usedPercentDisplay: `${shortPercent}%`, limitReached: false })
    const weekly = quotaWindow(6 * 1440 + 7 * 60, {
      key: 'weekly',
      role: 'secondary',
      windowSeconds: 604_800,
      labelDisplay: 'Codex 7天',
      windowLabelDisplay: '7天',
      usedPercent: weeklyPercent,
      usedPercentDisplay: `${weeklyPercent}%`,
      limitReached: false,
    })
    const { root } = mountComponent(context, 'AccountQuotaSummaryCell/index.vue', {
      account: quotaAccount([weekly, short]),
    })
    assertSummaryRows(root, [
      { window: short, code: '5H', countdown: '2h 10m' },
      { window: weekly, code: '7D', countdown: '6d 7h' },
    ])
    const header = root.children[0].children.find(node => node.tag === 'div')
    assert.match(textContent(header), /12\.5K.*Tokens.*OAuth/)
    assert.doesNotMatch(textContent(header), /使用率|%/)
    assert.doesNotMatch(textContent(root), /使用率/)
    assert.ok(textContent(root).includes('消费：$12.50'))
    assert.doesNotMatch(textContent(root), /预计总额/, 'quota percentages and local costs alone are not a paired forecast')
  })
}

test('real SummaryCell distinguishes visible zero from unknown usage with or without reset times', (context) => {
  for (const resetAt of [quotaWindow(25).resetAt, null]) {
    const zero = quotaWindow(25, { usedPercent: 0, usedPercentDisplay: '0%', limitReached: false, resetAt })
    const unknown = quotaWindow(25, {
      key: 'weekly',
      role: 'secondary',
      windowSeconds: 604_800,
      labelDisplay: 'Codex 7天',
      windowLabelDisplay: '7天',
      usedPercent: null,
      usedPercentDisplay: '—',
      limitReached: false,
      resetAt,
    })
    const { root } = mountComponent(context, 'AccountQuotaSummaryCell/index.vue', {
      account: quotaAccount([unknown, zero]),
    })
    const rows = assertSummaryRows(root, [
      { window: zero, code: '5H', countdown: resetAt ? '25m' : null },
      { window: unknown, code: '7D', countdown: resetAt ? '25m' : null },
    ])
    const [zeroBar, unknownBar] = rows.map(row => descendants(row, node => node.props.role === 'progressbar')[0])
    assert.equal(zeroBar.children[0].props.style.minWidth, '0')
    assert.equal(unknownBar.children[0].props.style.minWidth, '0')
    assert.match(zeroBar.children[0].props.class, /bg-cp-success/)
    assert.match(unknownBar.children[0].props.class, /bg-cp-border/)
    assert.doesNotMatch(textContent(root), /预计总额|使用率/)
  }
})

test('real SummaryCell shows every known percentage without local requests or cost statistics', (context) => {
  for (const requestCount of [0, null, undefined]) {
    const short = quotaWindow(0.5, { usedPercent: 0, usedPercentDisplay: '0%', limitReached: false })
    const weekly = quotaWindow(6 * 1440, {
      key: 'weekly',
      role: 'secondary',
      windowSeconds: 604_800,
      labelDisplay: 'Codex 7天',
      windowLabelDisplay: '7天',
      usedPercent: 64,
      usedPercentDisplay: '64%',
      limitReached: false,
    })
    const { root } = mountComponent(context, 'AccountQuotaSummaryCell/index.vue', {
      account: quotaAccount([weekly, short], { requestCount, costs: [] }),
    })
    assertSummaryRows(root, [
      { window: short, code: '5H', countdown: '<1m' },
      { window: weekly, code: '7D', countdown: '6d 0h' },
    ])
    assert.match(textContent(root), /OAuth/)
    assert.match(textContent(root), /消费：—/)
    assert.doesNotMatch(textContent(root), /Tokens|预计总额|使用率|账号不可用/)
    assert.equal(descendants(root, node => node.props.title === '暂无统计数据，不代表账号不可用').length, 1)
  }
})

test('real SummaryCell preserves all unknown cycles initially and after losing upstream observations', async (context) => {
  const short = quotaWindow(130)
  const weekly = quotaWindow(6 * 1440, {
    key: 'weekly',
    role: 'secondary',
    windowSeconds: 604_800,
    labelDisplay: 'Codex 7天',
    windowLabelDisplay: '7天',
    usedPercent: 35,
    usedPercentDisplay: '35%',
    limitReached: false,
  })
  const unknown = [short, weekly].map(window => ({
    ...window,
    usedPercent: null,
    usedPercentDisplay: '—',
    limitReached: false,
    resetAt: null,
    resetAtDisplay: '—',
  }))
  for (const initialWindows of [unknown, [short, weekly]]) {
    const { root, props } = mountComponent(context, 'AccountQuotaSummaryCell/index.vue', {
      account: quotaAccount(initialWindows, { requestCount: 0, costs: [] }),
    })
    props.value = { account: quotaAccount(unknown, { requestCount: 0, costs: [] }) }
    await nextTick()
    assertSummaryRows(root, [
      { window: unknown[0], code: '5H', countdown: null },
      { window: unknown[1], code: '7D', countdown: null },
    ])
    assert.equal(descendants(root, node => node.tag === 'time').length, 0)
    assert.doesNotMatch(textContent(root), /0%|100%|待确认|已恢复|使用率/)
  }
})

test('real SummaryCell retains recent-model group selection and independent observed costs across updates', async (context) => {
  const generic = quotaWindow(130)
  const modelShort = quotaWindow(25, {
    key: 'model-primary',
    limitId: 'gpt-model',
    limitName: 'GPT Model',
    labelDisplay: 'GPT Model 5小时',
    usedPercent: 20,
    usedPercentDisplay: '20%',
    limitReached: false,
  })
  const modelWeekly = quotaWindow(6 * 1440, {
    ...modelShort,
    key: 'model-weekly',
    role: 'secondary',
    windowSeconds: 604_800,
    labelDisplay: 'GPT Model 7天',
    windowLabelDisplay: '7天',
    usedPercent: 50,
    usedPercentDisplay: '50%',
    resetAt: quotaWindow(6 * 1440).resetAt,
  })
  const windows = [generic, modelWeekly, modelShort]
  const account = quotaAccount(windows, {
    costs: [
      { currency: 'EUR', estimatedAmount: '99.00', estimatedAmountDisplay: 'EUR 99.00' },
      { currency: 'usd', estimatedAmount: '12.50', estimatedAmountDisplay: '$12.50' },
    ],
    models: [
      { model: 'codex', lastUsedAt: '2026-09-12T01:00:00Z' },
      { model: 'GPT Model', lastUsedAt: '2026-09-13T01:00:00Z' },
    ],
  })
  const snapshot = JSON.stringify(account)
  const { root, props } = mountComponent(context, 'AccountQuotaSummaryCell/index.vue', { account })
  assertSummaryRows(root, [
    { window: modelShort, code: '5H', countdown: '25m' },
    { window: modelWeekly, code: '7D', countdown: '6d 0h' },
  ])
  assert.ok(textContent(root).includes('消费：$12.50'))
  assert.doesNotMatch(textContent(root), /预计总额/)
  assert.doesNotMatch(textContent(root), /使用率|EUR 99\.00/)

  const updatedShort = { ...modelShort, usedPercent: 80, usedPercentDisplay: '80%' }
  const updatedWeekly = { ...modelWeekly, usedPercent: 10, usedPercentDisplay: '10%' }
  props.value = {
    account: { ...account, quota: { windows: [updatedShort, generic, updatedWeekly] } },
  }
  await nextTick()
  assertSummaryRows(root, [
    { window: updatedShort, code: '5H', countdown: '25m' },
    { window: updatedWeekly, code: '7D', countdown: '6d 0h' },
  ])
  assert.ok(textContent(root).includes('消费：$12.50'))
  assert.doesNotMatch(textContent(root), /预计总额/)

  props.value = { account: { ...account, usage: { ...account.usage, models: [] } } }
  await nextTick()
  assertSummaryRows(root, [{ window: generic, code: '5H', countdown: '2h 10m' }])
  assert.ok(textContent(root).includes('消费：$12.50'))
  assert.doesNotMatch(textContent(root), /预计总额/)
  assert.equal(JSON.stringify(account), snapshot)
})

test('real SummaryCell never invents inline forecasts from quota percentages or local costs', (context) => {
  for (const [percent, amount] of [
    [0, '12.50'],
    [null, '12.50'],
    [50, undefined],
    [50, 'invalid'],
    [50, 'Infinity'],
    [50, '0.00'],
    [50, '12.50'],
  ]) {
    const window = quotaWindow(130, { usedPercent: percent, usedPercentDisplay: percent === null ? '—' : `${percent}%`, limitReached: false })
    const costs = amount === undefined
      ? []
      : [{ currency: 'USD', estimatedAmount: amount, estimatedAmountDisplay: `$${amount}` }]
    const { root } = mountComponent(context, 'AccountQuotaSummaryCell/index.vue', {
      account: quotaAccount([window], { costs }),
    })
    assertSummaryRows(root, [{ window, code: '5H', countdown: '2h 10m' }])
    assert.ok(textContent(root).includes(`消费：${costs[0]?.estimatedAmountDisplay ?? '—'}`))
    assert.doesNotMatch(textContent(root), /预计总额/)
    assert.doesNotMatch(textContent(root), /使用率/)
  }
})

for (const [label, windows, usage, expectedCost] of [
  ['recorded costs', [quotaWindow()], {}, '$12.50'],
  ['zero recorded costs', [quotaWindow()], { costs: [{ currency: 'USD', estimatedAmount: '0.00', estimatedAmountDisplay: '$0.00' }] }, '$0.00'],
  ['missing costs', [quotaWindow()], { costs: [] }, '—'],
  ['no local requests or costs', [quotaWindow()], { requestCount: 0, costs: [] }, '—'],
  ['no windows but recorded costs', [], {}, null],
  ['no windows or local statistics', [], { requestCount: 0, costs: [] }, null],
]) {
  test(`real SummaryCell keeps an available forecast footer with ${label}`, (context) => {
    const { root } = mountComponent(context, 'AccountQuotaSummaryCell/index.vue', {
      account: quotaAccount(windows, usage),
    })
    assertSummaryFooter(root, expectedCost)
    assert.doesNotMatch(textContent(root), /预计总额/)
    if (windows.length === 0) {
      assert.match(textContent(root), /额度待观测/)
      assert.doesNotMatch(textContent(root), /Tokens/)
      assert.equal(descendants(root, node => node.props.role === 'progressbar').length, 0)
    }
  })
}

test('real SummaryCell emits only explicit forecast clicks with the current account and preserves quota details', async (context) => {
  const events = []
  const first = quotaAccount([quotaWindow(130)])
  const { root, props } = mountComponent(context, 'AccountQuotaSummaryCell/index.vue', {
    account: first,
    onForecastRequested: account => events.push(account),
  })
  assert.deepEqual(events, [], 'rendering the account must not request a forecast')
  let stopped = 0
  const click = {
    stopPropagation: () => { stopped += 1 },
  }
  forecastButton(root).props.onClick(click)
  assert.equal(events.length, 1)
  assert.equal(events[0], props.value.account)
  assert.equal(stopped, 1, 'forecast clicks must not toggle the enclosing account row')

  const nextWindow = quotaWindow(25, { usedPercent: 0, usedPercentDisplay: '0%', limitReached: false })
  props.value = { ...props.value, account: quotaAccount([nextWindow], { requestCount: 0, costs: [] }) }
  clock.value = new Date(now + 60_000)
  await nextTick()
  assert.equal(events.length, 1, 'account refreshes and clock ticks must not request forecasts')
  assertSummaryRows(root, [{ window: nextWindow, code: '5H', countdown: '24m' }])
  forecastButton(root).props.onClick(click)
  assert.equal(events.length, 2)
  assert.equal(events[1], props.value.account)
  assert.notEqual(events[1], events[0])
  assert.equal(stopped, 2)
  assert.doesNotMatch(textContent(root), /预计总额/)

  for (const usage of [{}, { requestCount: 0, costs: [] }]) {
    props.value = { ...props.value, account: quotaAccount([], usage) }
    clock.value = new Date(clock.value.getTime() + 60_000)
    const priorEventCount = events.length
    await nextTick()
    assert.equal(events.length, priorEventCount, 'losing windows and clock ticks must not request forecasts')
    assert.match(textContent(root), /额度待观测/)
    assertSummaryFooter(root, null).props.onClick(click)
    assert.equal(events.length, priorEventCount + 1)
    assert.equal(events.at(-1), props.value.account)
    assert.equal(stopped, events.length, 'fallback forecast clicks must still stop row propagation')
  }

  props.value = { ...props.value, account: first }
  const priorEventCount = events.length
  await nextTick()
  assert.equal(events.length, priorEventCount, 'restoring quota windows must not request forecasts')
  assertSummaryFooter(root, '$12.50').props.onClick(click)
  assert.equal(events.length, priorEventCount + 1)
  assert.equal(events.at(-1), props.value.account)
  assert.equal(stopped, events.length)
})
