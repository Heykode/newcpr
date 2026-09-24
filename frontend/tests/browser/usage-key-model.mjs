import assert from 'node:assert/strict'
import { mkdir, writeFile } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'
import { isolateNetwork } from './usage-columns.mjs'

const endpoint = '/api/admin/usage/insights/diagnostics'
const failureMessage = '模拟诊断加载失败'
const timestamp = '2026-09-24T12:00:00Z'
const moneyCases = [
  { amount: '1234567.89', display: '$1,234,567.89', partial: false },
  { amount: '0', display: '$0.00', partial: true },
  { amount: null, display: '—', partial: true },
  { amount: '0.0042', display: '$0.0042', partial: false },
  { amount: '9999999999.99', display: '$9,999,999,999.99', partial: true },
]

function diagnosticPage(query) {
  const dimension = query.dimension
  const currentPage = Number(query.currentPage || 1)
  const offset = (currentPage - 1) * 20
  const items = Array.from({ length: dimension === 'keyModel' ? (currentPage === 1 ? 20 : 4) : 2 }, (_, index) => {
    const number = offset + index
    const keyName = number === 0
      ? `Synthetic team → ${'long-key-name-'.repeat(5)}`
      : number === 1 || number === 2 ? 'Same display name' : `Synthetic key ${number}`
    const model = number === 0 ? `synthetic-model-${'long-model-name-'.repeat(3)}` : `synthetic-model-${number % 3}`
    const money = moneyCases[number] || { amount: '12.34', display: '$12.34', partial: false }
    return {
      key: JSON.stringify([`fixture-key-${number}`, model]),
      name: `[${query.provider || 'all'}] ${keyName} → ${model}`,
      requestCount: 1000 - number,
      successCount: 998 - number,
      errorCount: 1,
      errorRate: 0.001,
      requestShare: 0.1,
      averageLatencyMs: 250,
      latencyP95Ms: 400,
      firstTokenP95Ms: 100,
      nonCompletionCount: 1,
      nonCompletionRate: 0.001,
      retryCount: 1,
      retryRate: 0.001,
      // Deliberately unlike the server order, to catch client-side risk sorting.
      impactScore: number / 100,
      estimatedCost: money.amount,
      costIncomplete: money.partial,
      attemptCount: 1001 - number,
      totalTokens: 1234 + number,
    }
  })
  return { dimension, currentPage, pageSize: dimension === 'keyModel' ? 20 : 100, hasMore: dimension === 'keyModel' && currentPage === 1, items }
}

function fixture() {
  const state = {
    calls: [],
    blocked: [],
    api: [],
    errors: [],
    layoutErrors: [],
    measurements: [],
    lifecyclePassed: false,
    closedViteFallbacks: 0,
    failNext: false,
    held: null,
    holdNext() {
      let release
      let start
      let finish
      const gate = {
        released: new Promise(resolve => release = resolve),
        started: new Promise(resolve => start = resolve),
        done: new Promise(resolve => finish = resolve),
        release: () => release(),
        start: request => start(request),
        finish: () => finish(),
      }
      state.held = gate
      return gate
    },
    async route(route, base) {
      const request = route.request()
      const url = new URL(request.url())
      const path = url.pathname.replace(/^\/dev(?=\/api\/)/, '')
      if (url.origin !== base.origin || path !== endpoint)
        return route.fallback()
      const query = Object.fromEntries(url.searchParams)
      try {
        assert.equal(request.method(), 'GET')
        assert(query.startTime && query.endTime)
        assert(Number.isFinite(Date.parse(query.startTime)) && Number.isFinite(Date.parse(query.endTime)))
        const expectedKeys = ['dimension', 'endTime', 'startTime']
        if (query.provider)
          expectedKeys.push('provider')
        if (query.dimension === 'keyModel') {
          expectedKeys.push('currentPage', 'pageSize')
          assert.equal(query.pageSize, '20')
          assert(['1', '2'].includes(query.currentPage), 'only adjacent valid pages are requested')
        }
        assert.deepEqual(Object.keys(query).sort(), expectedKeys.sort())
        const gate = state.held
        if (gate) {
          state.held = null
          gate.start(request)
          await gate.released
        }
        try {
          if (state.failNext) {
            state.failNext = false
            return await route.fulfill({ status: 503, json: { code: 503, message: failureMessage, data: null } })
          }
          return await route.fulfill({ json: { code: 200, message: 'ok', data: diagnosticPage(query) } })
        }
        finally {
          gate?.finish()
        }
      }
      catch (error) {
        state.errors.push(`${path}: ${error.message}`)
        return route.fulfill({ status: 500, json: { code: 500, message: 'Fixture contract violation', data: null } })
      }
    },
  }
  return state
}

function apiCounts(state) {
  return Object.fromEntries([
    '/api/admin/usage/records',
    '/api/admin/usage/records/summary',
    '/api/admin/usage/insights/overview',
  ].map(path => [path, state.calls.filter(call => call.path === path).length]))
}

function diagnosticsCalls(state) {
  return state.calls.filter(call => call.path === endpoint)
}

async function bounded(promise, label) {
  let timeout
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timeout = setTimeout(() => reject(new Error(`Timed out: ${label}`)), 15_000)
      }),
    ])
  }
  finally {
    clearTimeout(timeout)
  }
}

async function requestFrom(page, action) {
  const [response] = await Promise.all([
    page.waitForResponse(response => new URL(response.url()).pathname.endsWith(endpoint)),
    action(),
  ])
  return response
}

async function choose(page, select, label) {
  await select.click()
  const listbox = page.locator(`#${await select.getAttribute('aria-controls')}`)
  await listbox.waitFor()
  const bounds = await listbox.boundingBox()
  assert(bounds && bounds.x >= 0 && bounds.x + bounds.width <= page.viewportSize().width + 1, 'filter menu fits viewport')
  await listbox.getByRole('option', { name: label, exact: true }).click()
}

async function clearNotices(page) {
  const buttons = page.getByRole('button', { name: /^关闭(成功|警告|失败|信息)通知$/ })
  while (await buttons.count()) {
    try {
      await buttons.first().click({ timeout: 1000 })
    }
    catch (error) {
      if (await buttons.count())
        throw error
    }
  }
}

async function assertPage(card, query) {
  const expected = diagnosticPage(query)
  const nav = card.getByRole('navigation', { name: 'Key 模型分页' })
  await nav.getByText(`第 ${expected.currentPage} 页`, { exact: true }).waitFor()
  await card.locator('tbody tr[data-row-key]').first().waitFor()
  assert.deepEqual(await card.locator('tbody tr[data-row-key]').evaluateAll(rows => rows.map(row => row.dataset.rowKey)), expected.items.map(item => item.key), 'server pagination order and pair identities are preserved')
  assert.equal(await nav.getByRole('button', { name: '上一页', exact: true }).isDisabled(), expected.currentPage === 1)
  assert.equal(await nav.getByRole('button', { name: '下一页', exact: true }).isDisabled(), !expected.hasMore)
  assert.equal(await card.getByRole('columnheader', { name: '风险分', exact: true }).count(), 0)
  await card.getByRole('columnheader', { name: 'Token', exact: true }).waitFor()
  const rows = card.locator('tbody tr[data-row-key]')
  for (let index = 0; index < Math.min(expected.items.length, moneyCases.length); index++) {
    const item = expected.items[index]
    const model = JSON.parse(item.key)[1]
    const nameCell = rows.nth(index).locator('[data-column-key="nameDisplay"]')
    assert.equal(await nameCell.locator('code').first().textContent().then(text => text.trim()), item.name.slice(0, -` → ${model}`.length))
    assert.equal(await nameCell.locator('code').last().textContent().then(text => text.trim()), model)
    const cost = rows.nth(index).locator('[data-column-key="estimatedCost"] span').first()
    const expectedMoney = expected.currentPage === 1 ? moneyCases[index].display : '$12.34'
    assert.equal(await cost.locator('span').textContent().then(text => text.trim()), expectedMoney, 'full monetary value, including zero and unknown')
    assert.equal(await cost.getAttribute('title'), item.costIncomplete ? '部分请求费用未知' : null)
    assert.equal(await cost.getByText('部分', { exact: true }).count(), item.costIncomplete ? 1 : 0)
  }
}

async function snapshot(page, card, state, output, width, phase, column = 'nameDisplay', rowIndex = 0) {
  await card.scrollIntoViewIfNeeded()
  const cell = card.locator('tbody tr[data-row-key]').nth(rowIndex).locator(`[data-column-key="${column}"]`)
  if (await cell.count())
    await cell.scrollIntoViewIfNeeded()
  await page.evaluate(() => document.fonts.ready)
  await page.screenshot({ path: `${output}/${phase}-${width}.png`, fullPage: true })
  const measurement = await card.evaluate((element) => {
    const errors = []
    const card = element.getBoundingClientRect()
    if (document.documentElement.scrollWidth > innerWidth)
      errors.push('document has horizontal overflow')
    if (card.left < 0 || card.right > innerWidth + 1 || element.scrollWidth > element.clientWidth + 1)
      errors.push('diagnostic card has horizontal overflow')
    for (const control of element.querySelectorAll('header, nav, button')) {
      const rect = control.getBoundingClientRect()
      if (rect.width && (rect.left < card.left - 1 || rect.right > card.right + 1))
        errors.push('diagnostic header or pagination overflows its card')
    }
    const costs = []
    for (const cell of element.querySelectorAll('td[data-column-key="estimatedCost"]')) {
      const amount = cell.querySelector('span > span')
      const range = document.createRange()
      range.selectNodeContents(amount)
      const bounds = range.getBoundingClientRect()
      const container = cell.getBoundingClientRect()
      const style = getComputedStyle(cell)
      const left = container.left + Number.parseFloat(style.paddingLeft)
      const right = container.right - Number.parseFloat(style.paddingRight)
      const value = amount.textContent.trim()
      const fits = bounds.left >= left - 1 && bounds.right <= right + 1
      if (!fits)
        errors.push(`amount exceeds its own cell: ${value}`)
      const badge = cell.querySelector('small')
      if (badge && badge.getBoundingClientRect().top < bounds.bottom - 1)
        errors.push(`partial-cost badge must remain below its amount: ${value}`)
      costs.push({ value, textWidth: bounds.width, availableWidth: right - left, fits })
    }
    for (const code of element.querySelectorAll('td[data-column-key="nameDisplay"] code')) {
      if (code.scrollWidth > code.clientWidth + 1 || getComputedStyle(code).whiteSpace === 'nowrap')
        errors.push('Key or model name is clipped instead of wrapping')
    }
    return { errors: [...new Set(errors)], costs }
  })
  state.measurements.push({ phase, ...measurement })
  state.layoutErrors.push(...measurement.errors.map(error => `${phase}: ${error}`))
}

async function runViewport(page, state, base, output, width) {
  await page.clock.setFixedTime(new Date(timestamp))
  const initial = await requestFrom(page, () => page.goto(new URL('/usage', base).href))
  assert.equal(initial.status(), 200)
  const card = page.getByRole('article').filter({ has: page.getByRole('heading', { name: '热点诊断', exact: true }) })
  const dimension = card.getByRole('combobox', { name: '诊断维度', exact: true })
  const nav = card.getByRole('navigation', { name: 'Key 模型分页' })
  const next = nav.getByRole('button', { name: '下一页', exact: true })
  const previous = nav.getByRole('button', { name: '上一页', exact: true })
  const provider = page.getByRole('radiogroup', { name: '按平台筛选', exact: true })
  const timeRange = page.locator('header').filter({ has: page.getByRole('heading', { name: '使用统计', exact: true }) }).getByRole('combobox')
  const capture = (phase, column, rowIndex) => snapshot(page, card, state, output, width, phase, column, rowIndex)
  await card.getByRole('columnheader', { name: '风险分', exact: true }).waitFor()
  assert.equal(await nav.count(), 0)
  assert.equal(diagnosticsCalls(state).at(-1).query.dimension, 'model')
  const baseline = apiCounts(state)
  assert(Object.values(baseline).every(count => count === 1))

  await requestFrom(page, () => choose(page, dimension, 'Key × 模型'))
  let query = diagnosticsCalls(state).at(-1).query
  assert.equal(query.currentPage, '1')
  await assertPage(card, query)
  assert.deepEqual(apiCounts(state), baseline, 'dimension change refreshes diagnostics only')
  await capture('first-page-names')
  await capture('first-page-money', 'estimatedCost')
  await capture('zero-partial-money', 'estimatedCost', 1)
  await capture('unknown-partial-money', 'estimatedCost', 2)
  await capture('maximum-partial-money', 'estimatedCost', 4)

  const gate = state.holdNext()
  try {
    await next.click()
    await bounded(gate.started, 'page request starts')
    await card.getByText('正在加载热点诊断数据', { exact: true }).waitFor()
    assert.equal(await next.isDisabled(), true)
    assert.equal(await previous.isDisabled(), true)
    assert.equal(await dimension.isDisabled(), true)
    assert.deepEqual(apiCounts(state), baseline, 'pending pagination cannot refresh other analytics')
    await capture('page-loading')
  }
  finally {
    gate.release()
  }
  await bounded(gate.done, 'page response completes')
  query = { ...query, currentPage: '2' }
  await assertPage(card, query)
  assert.deepEqual(diagnosticsCalls(state).at(-1).query, query, 'pagination keeps the same range and provider')
  assert.deepEqual(apiCounts(state), baseline)
  await capture('last-page', 'estimatedCost')

  await requestFrom(page, () => previous.click())
  query = { ...query, currentPage: '1' }
  await assertPage(card, query)
  const beforeFailure = diagnosticsCalls(state).length
  state.failNext = true
  assert.equal((await requestFrom(page, () => next.click())).status(), 503)
  await page.getByRole('alert').filter({ hasText: failureMessage }).waitFor()
  await assertPage(card, query)
  assert.equal(await dimension.isDisabled(), false)
  assert.equal(diagnosticsCalls(state).length, beforeFailure + 1, 'failed reads wait for an explicit retry')
  await capture('failure-retry-ready')
  await clearNotices(page)
  await requestFrom(page, () => next.click())
  query = { ...query, currentPage: '2' }
  await assertPage(card, query)
  assert.equal(diagnosticsCalls(state).length, beforeFailure + 2)
  assert.deepEqual(apiCounts(state), baseline, 'page retry remains local to diagnostics')
  await capture('retry-succeeded')

  await requestFrom(page, () => provider.getByRole('radio', { name: 'OpenAI', exact: true }).click())
  query = { ...query, currentPage: '1', provider: 'openai' }
  assert.deepEqual(diagnosticsCalls(state).at(-1).query, query)
  await assertPage(card, query)
  assert.deepEqual(apiCounts(state), Object.fromEntries(Object.entries(baseline).map(([key, value]) => [key, value + 1])))
  await capture('provider-reset')
  await requestFrom(page, () => next.click())
  await assertPage(card, { ...query, currentPage: '2' })

  await requestFrom(page, () => choose(page, timeRange, '最近 7 天'))
  const rangeQuery = diagnosticsCalls(state).at(-1).query
  assert.equal(rangeQuery.currentPage, '1')
  assert.equal(rangeQuery.provider, 'openai')
  assert.equal(Date.parse(query.startTime) - Date.parse(rangeQuery.startTime), 6 * 24 * 60 * 60 * 1000)
  assert.equal(rangeQuery.endTime, query.endTime)
  query = rangeQuery
  await assertPage(card, query)
  await capture('time-reset')
  await requestFrom(page, () => next.click())
  await assertPage(card, { ...query, currentPage: '2' })

  const beforeDimension = apiCounts(state)
  await requestFrom(page, () => choose(page, dimension, '账号'))
  await card.getByRole('columnheader', { name: '风险分', exact: true }).waitFor()
  assert.equal(await nav.count(), 0)
  const accountQuery = diagnosticsCalls(state).at(-1).query
  assert.equal(accountQuery.dimension, 'account')
  assert.equal(accountQuery.currentPage, undefined)
  assert.equal(accountQuery.pageSize, undefined)
  await requestFrom(page, () => choose(page, dimension, 'Key × 模型'))
  await assertPage(card, query)
  assert.deepEqual(apiCounts(state), beforeDimension)
  await capture('dimension-reset')

  const stale = state.holdNext()
  let staleRequest
  try {
    await next.click()
    staleRequest = await bounded(stale.started, 'stale page request starts')
    const cancelled = page.waitForEvent('requestfailed', { predicate: request => request === staleRequest })
    await requestFrom(page, () => provider.getByRole('radio', { name: 'xAI', exact: true }).click())
    await cancelled
    query = { ...query, provider: 'xai', currentPage: '1' }
    await assertPage(card, query)
  }
  finally {
    stale.release()
  }
  await bounded(stale.done, 'cancelled page fixture finishes')
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))
  await assertPage(card, query)
  await capture('stale-page-rejected')

  await requestFrom(page, () => next.click())
  query = { ...query, currentPage: '2' }
  await assertPage(card, query)
  for (const filter of ['provider', 'time']) {
    const expectedQuery = {
      ...query,
      currentPage: '1',
      ...(filter === 'provider'
        ? { provider: 'openai' }
        : { startTime: new Date(Date.parse(query.startTime) - 23 * 24 * 60 * 60 * 1000).toISOString() }),
    }
    const callsBefore = diagnosticsCalls(state).length
    const analyticsBefore = apiCounts(state)
    state.failNext = true
    const failure = await requestFrom(page, () => filter === 'provider'
      ? provider.getByRole('radio', { name: 'OpenAI', exact: true }).click()
      : choose(page, timeRange, '最近 30 天'))
    assert.equal(failure.status(), 503)
    assert.deepEqual(diagnosticsCalls(state).at(-1).query, expectedQuery, `${filter} change requests the new first page`)
    await page.getByRole('alert').filter({ hasText: failureMessage }).waitFor()
    await card.getByText('暂无诊断数据', { exact: true }).waitFor()
    await nav.getByText('第 1 页', { exact: true }).waitFor()
    assert.equal(await card.locator('tbody tr[data-row-key]').count(), 0, `${filter} failure clears old results`)
    assert.equal(await previous.isDisabled(), true, `${filter} failure clears the old previous page`)
    assert.equal(await next.isDisabled(), true, `${filter} failure clears the old next page`)
    assert.equal(await dimension.isDisabled(), false, 'dimension remains available for an explicit requery')
    // Send native pointer clicks without waiting for disabled controls to become enabled.
    await previous.click({ force: true })
    await next.click({ force: true })
    await capture(`${filter}-first-page-failed`)
    assert.equal(diagnosticsCalls(state).length, callsBefore + 1, 'disabled old pagination cannot request or retry')
    assert.deepEqual(apiCounts(state), Object.fromEntries(Object.entries(analyticsBefore).map(([key, value]) => [key, value + 1])))
    await clearNotices(page)

    const beforeRequery = apiCounts(state)
    await requestFrom(page, () => choose(page, dimension, '模型'))
    await nav.waitFor({ state: 'hidden' })
    assert.deepEqual(diagnosticsCalls(state).at(-1).query, {
      dimension: 'model',
      startTime: expectedQuery.startTime,
      endTime: expectedQuery.endTime,
      provider: expectedQuery.provider,
    }, 'explicit requery retains the current provider and time range')
    await requestFrom(page, () => choose(page, dimension, 'Key × 模型'))
    query = expectedQuery
    assert.deepEqual(diagnosticsCalls(state).at(-1).query, query)
    await assertPage(card, query)
    assert.equal(diagnosticsCalls(state).length, callsBefore + 3)
    assert.deepEqual(apiCounts(state), beforeRequery, 'dimension requery stays local to diagnostics')
    await capture(`${filter}-explicit-requery`)

    await requestFrom(page, () => next.click())
    query = { ...query, currentPage: '2' }
    assert.deepEqual(diagnosticsCalls(state).at(-1).query, query)
    await assertPage(card, query)
    assert.equal(diagnosticsCalls(state).length, callsBefore + 4)
    assert.deepEqual(apiCounts(state), beforeRequery)
    await capture(`${filter}-requery-next-page`)
  }
  assert.deepEqual(state.errors, [])
  assert.deepEqual(state.blocked, [], 'all traffic must be local fixtures or source assets')
  assert(state.calls.every(call => call.method === 'GET'), 'analytics must not make mutations')
  state.lifecyclePassed = true
  assert.deepEqual(state.layoutErrors, [], `${width}px layout violations after completed lifecycle`)
}

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-usage-key-model-qa'
  await mkdir(output, { recursive: true })
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    cacheDir: `${output}/vite-cache`,
    server: { host: '127.0.0.1', port: 0, hmr: false, ws: false },
    plugins: [{
      name: 'isolated-usage-key-model-qa',
      configResolved(config) { config.server.proxy = {} },
    }],
  })
  let browser
  try {
    await server.listen()
    const base = new URL(`http://127.0.0.1:${server.httpServer.address().port}`)
    browser = await chromium.launch({
      headless: true,
      args: ['--disable-background-networking', '--no-proxy-server'],
      ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
    })
    const failures = []
    for (const width of [1440, 390, 320]) {
      const context = await browser.newContext({
        viewport: { width, height: 1000 },
        colorScheme: 'light',
        reducedMotion: 'reduce',
        timezoneId: 'UTC',
        serviceWorkers: 'block',
      })
      const state = fixture()
      const page = await context.newPage()
      page.setDefaultTimeout(15_000)
      page.on('pageerror', error => state.errors.push(error.message))
      page.on('request', (request) => {
        const url = new URL(request.url())
        const path = url.pathname.replace(/^\/dev(?=\/api\/)/, '')
        if (path.startsWith('/api/'))
          state.calls.push({ path, method: request.method(), query: Object.fromEntries(url.searchParams) })
      })
      await isolateNetwork(context, base, state)
      // The installed Vite client attempts this fallback even with HMR/WS disabled.
      await context.routeWebSocket(url => url.protocol === 'ws:'
        && url.hostname === base.hostname && url.port === '0' && url.pathname === '/'
        && url.searchParams.get('token') === server.config.webSocketToken, (socket) => {
        state.closedViteFallbacks++
        socket.close()
      })
      await page.route('**/*', route => state.route(route, base))
      try {
        await runViewport(page, state, base, output, width)
        process.stdout.write(`PASS ${width}px: pagination/filter-reset/filter-failure/explicit-requery/retry/stale-response/full-money/layout; synthetic APIs only.\n`)
      }
      catch (error) {
        await page.screenshot({ path: `${output}/failure-${width}.png`, fullPage: true })
        failures.push(new Error(`${width}px: ${error.message}; fixture errors: ${JSON.stringify(state.errors)}`, { cause: error }))
      }
      finally {
        state.held?.release()
        await writeFile(`${output}/report-${width}.json`, `${JSON.stringify({
          calls: state.calls,
          blocked: state.blocked,
          errors: state.errors,
          lifecyclePassed: state.lifecyclePassed,
          closedViteFallbacks: state.closedViteFallbacks,
          measurements: state.measurements,
        }, null, 2)}\n`)
        await context.close()
      }
    }
    if (failures.length)
      throw new AggregateError(failures, 'Usage Key/model browser regression failed')
    process.stdout.write(`Screenshots and geometry reports: ${output}\n`)
  }
  finally {
    await browser?.close()
    await server.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
