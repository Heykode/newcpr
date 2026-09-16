/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import { createSSRApp, defineComponent, h, ref } from 'vue'
import { compileScript, parse } from 'vue/compiler-sfc'
import { renderToString } from 'vue/server-renderer'

const require = createRequire(import.meta.url)
const now = Date.parse('2026-09-13T02:00:00Z')
const clock = ref(new Date(now))
const modules = new Map()
const popoverOpen = ref(false)
const popover = defineComponent({
  props: ['disabled', 'trigger', 'placement'],
  setup: (props, { slots }) => () => h('div', {
    'data-popover-disabled': String(props.disabled),
    'data-popover-trigger': props.trigger,
    'data-popover-placement': props.placement,
  }, [
    slots.trigger?.({ open: !props.disabled && popoverOpen.value }),
    !props.disabled && popoverOpen.value ? slots.default?.() : undefined,
  ]),
})
const scrollbar = defineComponent({
  props: ['maxHeight'],
  setup: (props, { slots }) => () => h('div', {
    'data-scrollbar-max-height': props.maxHeight,
  }, slots.default?.()),
})
// Linked icon packages can resolve a second Vue runtime outside this worktree.
const icons = Object.fromEntries(
  ['AlertTriangle', 'CircleCheck', 'Gauge', 'Power', 'Timer', 'Users', 'ShieldCheck', 'ChevronLeft', 'ChevronRight', 'RefreshCw', 'Info', 'Pin', 'PinOff']
    .map(name => [name, defineComponent({ setup: () => () => h('svg', { 'data-icon': name }) })]),
)

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
      if (name === '../composables/useGroupMonitor') {
        return { useGroupMonitor: () => ({
          page: ref(0),
          pins: ref([]),
          viewer: ref('test'),
          visible: ref([]),
          displayedCount: ref(0),
          totalPages: ref(1),
          records: ref(new Map()),
          stale: ref(false),
          loading: ref(false),
          error: ref(false),
          now: ref(now),
          togglePin() {},
          refreshNow() {},
        }) }
      }
      if (name === '@/components/base/BaseIconButton.vue')
        return defineComponent({ setup: (_, { slots }) => () => h('button', slots.default?.()) })
      if (name === '@/composables/useUiClock')
        return { useUiClock: () => clock }
      if (name === '@/components/base/BasePopover.vue')
        return popover
      if (name === '@/components/base/BaseScrollbar.vue')
        return scrollbar
      if (name === '@/components/base/BaseCard.vue' || name === '@/components/base/BaseMotionIcon.vue')
        return defineComponent({ setup: (_, { slots }) => () => h('section', slots.default?.()) })
      if (name === '@lucide/vue')
        return icons
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

const component = loadSource(new URL('../src/views/accounts/components/AccountStatusBadge/index.vue', import.meta.url)).default

test('overview merges total and normal without error or quota cards or inferring enabled counts', async () => {
  const overview = loadSource(new URL('../src/views/accounts/components/AccountOverviewCards.vue', import.meta.url)).default
  const html = await renderToString(createSSRApp(overview, {
    summary: { total: 4, normal: 1, quotaExhausted: 1, rateLimited: 1, disabled: 4, error: 1 },
    groups: [],
    groupsLoading: false,
  }))
  assert.match(html, /总账号\s*<\/p>[\s\S]*?<strong[^>]*>4<\/strong>/)
  assert.match(html, /正常 <strong[^>]*>1<\/strong>/)
  assert.doesNotMatch(html, /错误账号|额度受限|可参与调度|已停用 \/ 错误|待处理/)
})
const future = new Date(now + 75 * 60_000).toISOString()
const past = new Date(now - 60_000).toISOString()
const states = [
  { name: 'normal', props: { status: 'normal' }, label: '\u6B63\u5E38', text: 'text-cp-success-text', dot: 'bg-cp-success', detail: false },
  { name: 'quota exhausted', props: { status: 'quota_exhausted' }, label: '\u914D\u989D\u8017\u5C3D', text: 'text-cp-warning-text', dot: 'bg-cp-warning', detail: false },
  { name: 'rate limited without recovery time', props: { status: 'rate_limited' }, label: '\u9650\u6D41\u4E2D', text: 'text-cp-warning-text', dot: 'bg-cp-warning', detail: false },
  { name: 'rate limited with recovery time', props: { status: 'rate_limited', rateLimitedUntil: future }, label: '\u9650\u6D41\u4E2D', text: 'text-cp-warning-text', dot: 'bg-cp-warning', detail: true },
  { name: 'disabled ignores future retry', props: { status: 'disabled', nextRefreshAt: future }, label: '\u5DF2\u505C\u7528', text: 'text-cp-text-secondary', dot: 'bg-cp-text-quaternary', detail: false },
  { name: 'credential error', props: { status: 'error', errorReason: 'credential_invalid', errorMessage: 'upstream <blocked> & retry' }, label: '\u9519\u8BEF', text: 'text-cp-error-text', dot: 'bg-cp-error', detail: true },
  { name: 'OAuth backoff', props: { status: 'error', nextRefreshAt: future }, label: '\u9000\u907F\u4E2D', text: 'text-cp-warning-text', dot: 'bg-cp-warning', detail: true },
  { name: 'expired backoff returns to error', props: { status: 'error', nextRefreshAt: past }, label: '\u9519\u8BEF', text: 'text-cp-error-text', dot: 'bg-cp-error', detail: true },
]

function renderBadge(props) {
  return renderToString(createSSRApp(component, props))
}

function inlineMark(html) {
  const mark = html.match(/<span\s[^>]*\bdata-account-status-mark(?=[\s=>])[^>]*>/)?.[0]
  assert.ok(mark, html)
  return mark
}

for (const state of states) {
  test(`status alignment preserves ${state.name} labels, colors and popover semantics`, async () => {
    for (const open of [false, true]) {
      popoverOpen.value = open
      const left = await renderBadge(state.props)
      const explicitLeft = await renderBadge({ ...state.props, align: 'left' })
      const center = await renderBadge({ ...state.props, align: 'center' })
      assert.equal(left, explicitLeft, 'omitting align retains the existing left default')
      const leftMark = inlineMark(left)
      const centerMark = inlineMark(center)
      assert.doesNotMatch(leftMark, /\bjustify-center\b/)
      assert.match(centerMark, /\bjustify-center\b/)
      assert.equal(
        center.replace(centerMark, centerMark.replace(/\sjustify-center(?=[\s"])/, '')),
        left,
        'centering changes only the inline mark alignment, including inside an open popover',
      )
      for (const html of [left, center]) {
        assert.ok(inlineMark(html).includes(state.text), html)
        assert.ok(html.includes(`size-1.5 rounded-full ${state.dot}`), html)
        assert.ok(html.includes(`<span>${state.label}</span>`), html)
        assert.match(html, /data-popover-trigger="hover-click"/)
        assert.match(html, /data-popover-placement="top-start"/)
        assert.ok(html.includes(`data-popover-disabled="${!state.detail}"`), html)
        assert.equal(html.includes('<section'), open && state.detail)
        if (state.detail) {
          assert.match(html, /<button\s[^>]*type="button"[^>]*aria-label="[^"]+"[^>]*aria-expanded="(?:true|false)"[^>]*aria-haspopup="dialog"/)
          assert.ok(html.includes(`aria-expanded="${open}"`), html)
        }
        else {
          assert.doesNotMatch(html, /<button\b|aria-expanded|aria-haspopup|aria-label/)
        }
      }
    }
  })
}

test('status pill rendering remains unchanged for omitted, left and center alignment', async () => {
  for (const state of states) {
    for (const open of [false, true]) {
      popoverOpen.value = open
      const props = { ...state.props, variant: 'pill' }
      const original = await renderBadge(props)
      assert.equal(await renderBadge({ ...props, align: 'left' }), original)
      assert.equal(await renderBadge({ ...props, align: 'center' }), original)
      assert.ok(original.includes(state.label), original)
      assert.ok(original.includes(state.text), original)
      assert.doesNotMatch(original, /data-account-status-mark|size-1\.5 rounded-full/)
    }
  }
})

test('centered status retains recovery details, error escaping and bounded raw feedback', async () => {
  popoverOpen.value = true
  const error = await renderBadge({
    status: 'error',
    align: 'center',
    errorReason: 'credential_invalid',
    errorMessage: 'upstream <blocked> & retry',
  })
  assert.match(error, /upstream &lt;blocked&gt; &amp; retry/)
  assert.doesNotMatch(error, /<blocked>/)
  assert.match(error, /data-scrollbar-max-height="124px"/)
  assert.ok(error.includes('\u51ED\u636E\u65E0\u6548'), error)
  assert.ok(error.includes('\u5EFA\u8BAE\u64CD\u4F5C'), error)

  const limited = await renderBadge({ status: 'rate_limited', align: 'center', rateLimitedUntil: future })
  assert.ok(limited.includes('\u9884\u8BA1\u6062\u590D'), limited)
  assert.ok(limited.includes('\u5269\u4F59 1 \u5C0F\u65F6 15 \u5206'), limited)

  const backoff = await renderBadge({ status: 'error', align: 'center', nextRefreshAt: future })
  assert.ok(backoff.includes('OAuth \u5237\u65B0\u9000\u907F'), backoff)
  assert.ok(backoff.includes('\u4E0B\u6B21\u5C1D\u8BD5'), backoff)
})
