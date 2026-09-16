// Run against an already-started local frontend with QA_BASE_URL.
// PLAYWRIGHT_MODULE, CHROME_PATH and QA_OUTPUT_DIR follow the other browser runners.
import assert from 'node:assert/strict'
import { mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

const tables = [
  {
    id: 'success',
    tab: '成功记录',
    settings: '成功记录显示列',
    toolbar: '使用记录筛选与操作',
    refresh: '刷新使用记录',
    storage: 'codex-proxy:table-columns:usage-records',
    hidden: { key: 'model', label: '模型' },
    required: ['账号', '操作'],
    columns: ['accountEmail', 'provider', 'model', 'reasoningEffort', 'route', 'upstreamTransport', 'clientTransport', 'tokenDetails', 'billing', 'latency', 'createdAtDisplay', 'clientIp', 'userAgent', 'actions'],
  },
  {
    id: 'errors',
    tab: '错误排查',
    settings: '错误排查显示列',
    toolbar: '错误筛选与操作',
    refresh: '刷新错误明细',
    storage: 'codex-proxy:table-columns:ops-errors',
    hidden: { key: 'route', label: '端点' },
    required: ['账号', '错误', '操作'],
    columns: ['accountId', 'provider', 'message', 'upstreamSendState', 'model', 'route', 'createdAtDisplay', 'requestId', 'clientIp', 'userAgent', 'actions'],
  },
]

function fixtures() {
  const common = {
    provider: 'openai',
    authenticationKind: 'oauth',
    accountId: 'acct_usage_columns',
    accountName: 'Synthetic usage account',
    accountEmail: 'usage-columns@example.invalid',
    route: '/v1/responses',
    model: 'synthetic-model',
    requestedModel: 'synthetic-model',
    upstreamModel: 'synthetic-model-upstream',
    serviceTier: 'default',
    clientTransport: 'websocket',
    reasoningEffort: 'high',
    reasoningPreset: null,
    subagentKind: null,
    compact: false,
    latencyMs: 1234,
    clientIp: '192.0.2.10',
    userAgent: 'SyntheticUsageColumns/1.0 (Local browser fixture; no upstream requests)',
    createdAt: '2026-01-01T04:00:00Z',
    createdAtDisplay: '2026-01-01 12:00:00',
  }
  const success = [1, 2].map(index => ({
    ...common,
    id: `usage_columns_${index}`,
    upstreamTransport: 'http_sse',
    tokenDetails: Object.fromEntries(Object.entries({
      inputTokens: 120,
      outputTokens: 40,
      cachedTokens: 20,
      cacheWriteTokens: 0,
      reasoningTokens: 10,
      imageInputTokens: 0,
      imageOutputTokens: 0,
      totalTokens: 160,
    }).flatMap(([key, value]) => [[key, value], [`${key}Display`, String(value)]])),
    billing: null,
    latencyDetails: { firstTokenMs: 100, firstEventMs: 80 },
    firstTokenLatencyMs: 100,
  }))
  const errors = [1, 2].map(index => ({
    ...common,
    id: `ops_columns_${index}`,
    requestId: `req_columns_${index}`,
    clientApiKeyId: null,
    kind: 'request',
    operation: 'responses',
    protocol: 'openai',
    clientStatusCode: 503,
    upstreamStatusCode: 503,
    transport: 'http_sse',
    attemptIndex: 1,
    failureClass: 'upstream_error',
    upstreamSendState: 'sent',
    providerErrorCode: 'server_is_overloaded',
    responseId: null,
    upstreamRequestId: null,
    requestKind: null,
    message: 'Synthetic upstream unavailable; browser-only fixture',
    rawUpstreamError: null,
    metadata: {
      source: 'provider',
      component: 'synthetic',
      attemptId: null,
      accountLabel: null,
      continuationAffinityHash: null,
      continuationPreviousResponseIdHash: null,
      continuationUnavailableReason: null,
      upstreamConnectionId: null,
      upstreamConnectionExitReason: null,
      upstreamConnectionAgeMs: null,
      upstreamConnectionIdleMs: null,
      recoveryRequestId: null,
      recoveredAt: null,
      recoveryAttemptCount: 0,
      recoveryRetryDelayMs: null,
      recoveryTotalLatencyMs: null,
    },
  }))
  const overview = {
    granularity: '1d',
    health: {
      totalRequests: 4,
      successRequests: 2,
      failedRequests: 2,
      cancelledRequests: 0,
      incompleteRequests: 0,
      callerErrorRequests: 0,
      successRate: 0.5,
      completionRate: 0.5,
      requestChangeRate: null,
      successRateChange: null,
      points: [],
    },
    performance: {
      ...Object.fromEntries([
        'latencyP50Ms',
        'latencyP95Ms',
        'latencyP99Ms',
        'firstTokenP50Ms',
        'firstTokenP95Ms',
        'firstTokenP99Ms',
        'admissionDecisionP50Ms',
        'admissionDecisionP95Ms',
        'accountSelectionWaitP50Ms',
        'accountSelectionWaitP95Ms',
        'outputThroughputP10',
        'outputThroughputP50',
        'outputThroughputP90',
        'capacityUtilization',
        'capacityUtilizationP95',
      ].map(key => [key, null])),
      latencyCoverage: 0,
      firstTokenCoverage: 0,
      admissionDecisionCoverage: 0,
      accountSelectionWaitCoverage: 0,
      capacityCoverage: 0,
      points: [],
    },
    cost: {
      estimatedCost: null,
      standardCost: null,
      noCacheCost: null,
      cacheSavings: null,
      tierPremium: null,
      costPerRequest: null,
      costPerSuccessfulRequest: null,
      tokensPerRequest: 160,
      cachedTokenRate: 0.125,
      cacheHitRequestRate: 1,
      inputTokens: 240,
      outputTokens: 80,
      cachedTokens: 40,
      totalTokens: 320,
      points: [],
      coverage: { known: 0, partial: 0, unknown: 2, notBillable: 0 },
    },
  }
  return new Map([
    ['/api/admin/auth/status', { authenticated: true }],
    ['/api/admin/system/version', {
      version: 'synthetic',
      gitSha: 'synthetic',
      buildTime: common.createdAt,
      deploymentMode: 'local',
      deploymentModeLabel: 'Local fixture',
      updateChannel: 'stable',
      latestVersion: 'synthetic',
      hasUpdate: false,
      updateCached: true,
      updateWarning: null,
    }],
    ['/api/admin/usage/records', { items: success, currentPage: 1, pageSize: 10, total: success.length }],
    ['/api/admin/operations/errors', { items: errors, currentPage: 1, pageSize: 10, total: errors.length }],
    ['/api/admin/usage/records/summary', {
      totalRequests: '2',
      inputTokens: '240',
      outputTokens: '80',
      cachedTokens: '40',
      cacheWriteTokens: '0',
      totalTokens: '320',
      averageLatencyMs: '1234 ms',
    }],
    ['/api/admin/usage/insights/overview', overview],
    ['/api/admin/usage/insights/diagnostics', { dimension: 'model', items: [] }],
  ])
}

async function isolateNetwork(context, base, report) {
  const data = fixtures()
  await context.route('**/*', async (route) => {
    const request = route.request()
    const url = new URL(request.url())
    const method = request.method()
    const path = url.pathname.replace(/^\/dev(?=\/api\/)/, '')
    const signature = `${method} ${url.origin}${path}`
    if (url.origin !== base.origin || !['GET', 'HEAD'].includes(method)) {
      report.blocked.push(signature)
      return route.abort('blockedbyclient')
    }
    // Never let an unmocked API call reach Vite's backend proxy.
    if (data.has(path) && method === 'GET') {
      report.api.push(path)
      return route.fulfill({ json: { code: 200, message: 'ok', data: data.get(path) } })
    }
    const localAsset = /^\/(?:src|assets|node_modules|@vite|@id|@fs)\//.test(path)
      || path === '/favicon.ico' || path === '/favicon.svg'
    if ((request.isNavigationRequest() && path === '/usage') || localAsset)
      return route.continue()
    report.blocked.push(signature)
    return route.abort('blockedbyclient')
  })
  assert.equal(typeof context.routeWebSocket, 'function', 'Playwright must support WebSocket routing to enforce isolation')
  await context.routeWebSocket('**/*', (socket) => {
    const url = new URL(socket.url())
    const origin = `${url.protocol === 'wss:' ? 'https:' : 'http:'}//${url.host}`
    if (origin !== base.origin)
      report.blocked.push(`WS ${origin}${url.pathname}`)
    // No upstream socket is connected, including the local development HMR socket.
    socket.close()
  })
}

function activeTable(page) {
  return page.locator('table:visible')
}

async function assertColumns(page, table, hidden = false) {
  const expected = table.columns.filter(key => !hidden || key !== table.hidden.key)
  await activeTable(page).locator('tbody tr').first().waitFor()
  await page.waitForFunction((expected) => {
    const table = [...document.querySelectorAll('table')].find(element => element.getClientRects().length)
    const keys = [...(table?.querySelectorAll('thead th[data-column-key]') ?? [])].map(cell => cell.dataset.columnKey)
    return JSON.stringify(keys) === JSON.stringify(expected)
  }, expected)
  assert.equal(await activeTable(page).locator('tbody tr').count(), 2)
  for (const row of await activeTable(page).locator('tbody tr').all())
    assert.deepEqual(await row.locator('td[data-column-key]').evaluateAll(cells => cells.map(cell => cell.dataset.columnKey)), expected)
}

async function selectTable(page, table, hidden = false) {
  await page.getByRole('radiogroup', { name: '请求明细类型', exact: true })
    .getByRole('radio', { name: table.tab, exact: true })
    .click()
  await page.getByRole('button', { name: table.settings, exact: true }).waitFor()
  await assertColumns(page, table, hidden)
}

async function waitForFocus(page, locator) {
  await page.waitForFunction(element => document.activeElement === element, await locator.elementHandle())
  assert.ok(await locator.evaluate(element => element === document.activeElement))
}

async function openSettings(page, table) {
  const trigger = page.getByRole('button', { name: table.settings, exact: true })
  await trigger.scrollIntoViewIfNeeded()
  await trigger.focus()
  await page.keyboard.press('Enter')
  const dialog = page.getByRole('dialog', { name: table.settings, exact: true })
  await dialog.waitFor()
  assert.equal(await trigger.getAttribute('aria-expanded'), 'true')
  assert.equal(await trigger.getAttribute('aria-controls'), await dialog.getAttribute('id'))
  await waitForFocus(page, dialog.locator('input:not(:disabled)').first())
  for (const label of table.required) {
    const checkbox = dialog.getByRole('checkbox', { name: label, exact: true })
    assert.equal(await checkbox.isDisabled(), true, `${table.id}: required ${label}`)
    assert.equal(await checkbox.isChecked(), true, `${table.id}: required ${label}`)
  }
  return dialog
}

async function closeSettings(page, table) {
  await page.keyboard.press('Escape')
  await page.getByRole('dialog', { name: table.settings, exact: true }).waitFor({ state: 'hidden' })
  await waitForFocus(page, page.getByRole('button', { name: table.settings, exact: true }))
}

async function checkKeyboard(page, table) {
  await openSettings(page, table)
  await closeSettings(page, table)
  await openSettings(page, table)
  await page.keyboard.press('Shift+Tab')
  await page.getByRole('dialog', { name: table.settings, exact: true }).waitFor({ state: 'hidden' })
  await waitForFocus(page, page.getByRole('button', { name: table.settings, exact: true }))
  const dialog = await openSettings(page, table)
  const inputs = await dialog.locator('input:not(:disabled)').count()
  for (let index = 0; index < inputs; index += 1)
    await page.keyboard.press('Tab')
  await waitForFocus(page, dialog.getByRole('button', { name: '恢复默认', exact: true }))
  await page.keyboard.press('Tab')
  await dialog.waitFor({ state: 'hidden' })
  await waitForFocus(page, page.getByRole('button', { name: table.refresh, exact: true }))
}

async function assertPreference(page, table, hidden) {
  await page.waitForFunction(({ key, field, hidden }) => {
    const value = JSON.parse(localStorage.getItem(key) ?? '{}')
    return hidden ? value[field] === false : Object.keys(value).length === 0
  }, { key: table.storage, field: table.hidden.key, hidden })
}

async function geometry(page, table, panel = false) {
  const pageOverflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)
  assert.ok(pageOverflow <= 1, `Document overflows by ${pageOverflow}px`)
  const toolbar = page.getByRole('group', { name: table.toolbar, exact: true })
  const toolbarMetrics = await toolbar.evaluate((element) => {
    const rect = element.getBoundingClientRect()
    const ancestors = []
    let visibleLeft = 0
    let visibleRight = window.innerWidth
    for (let parent = element.parentElement; parent; parent = parent.parentElement) {
      const style = getComputedStyle(parent)
      if (style.overflowX === 'visible')
        continue
      const bounds = parent.getBoundingClientRect()
      const left = bounds.left + parent.clientLeft
      const right = left + parent.clientWidth
      visibleLeft = Math.max(visibleLeft, left)
      visibleRight = Math.min(visibleRight, right)
      ancestors.push({
        tag: parent.tagName,
        className: parent.className,
        left,
        right,
        scrollLeft: parent.scrollLeft,
        overflow: parent.scrollWidth - parent.clientWidth,
      })
    }
    return {
      left: rect.left,
      right: rect.right,
      width: window.innerWidth,
      overflow: element.scrollWidth - element.clientWidth,
      visibleLeft,
      visibleRight,
      ancestors,
    }
  })
  assert.ok(toolbarMetrics.left >= -1 && toolbarMetrics.right <= toolbarMetrics.width + 1 && toolbarMetrics.overflow <= 1, `Toolbar clipped: ${JSON.stringify(toolbarMetrics)}`)
  assert.ok(toolbarMetrics.left >= toolbarMetrics.visibleLeft - 1 && toolbarMetrics.right <= toolbarMetrics.visibleRight + 1, `Toolbar clipped by ancestor: ${JSON.stringify(toolbarMetrics)}`)
  assert.ok(toolbarMetrics.ancestors.every(ancestor => Math.abs(ancestor.scrollLeft) <= 1), `Unexpected horizontal ancestor scroll: ${JSON.stringify(toolbarMetrics)}`)
  const scrolling = await activeTable(page).evaluate((element) => {
    let parent = element.parentElement
    while (parent && !['auto', 'scroll'].includes(getComputedStyle(parent).overflowX))
      parent = parent.parentElement
    if (!parent)
      return null
    const rect = parent.getBoundingClientRect()
    return { left: rect.left, right: rect.right, width: window.innerWidth, clientWidth: parent.clientWidth, scrollWidth: parent.scrollWidth }
  })
  assert.ok(scrolling && scrolling.left >= -1 && scrolling.right <= scrolling.width + 1, `Table must scroll within the viewport: ${JSON.stringify(scrolling)}`)
  if (panel) {
    const dialog = page.getByRole('dialog', { name: table.settings, exact: true })
    await page.waitForFunction((id) => {
      const rect = document.getElementById(id)?.getBoundingClientRect()
      return rect && rect.width > 0 && rect.left >= 0 && rect.top >= 0
        && rect.right <= window.innerWidth + 1 && rect.bottom <= window.innerHeight + 1
    }, await dialog.getAttribute('id'))
    assert.ok(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth + 1), 'Column settings must not overflow horizontally')
  }
  return { pageOverflow, toolbar: toolbarMetrics, scrolling }
}

async function runScenario(browser, base, output, theme, width) {
  const name = `${theme}-${width}`
  const report = { scenario: name, api: [], blocked: [], pageErrors: [], failedResources: [], screenshots: [], geometry: [] }
  const context = await browser.newContext({
    viewport: { width, height: width < 500 ? 844 : 1000 },
    reducedMotion: 'reduce',
    serviceWorkers: 'block',
  })
  let page
  try {
    await isolateNetwork(context, base, report)
    page = await context.newPage()
    await page.emulateMedia({ colorScheme: theme })
    page.setDefaultTimeout(15_000)
    page.on('pageerror', error => report.pageErrors.push(error.message))
    page.on('response', (response) => {
      if (response.status() >= 400)
        report.failedResources.push({ path: new URL(response.url()).pathname, status: response.status() })
    })
    const ready = async () => {
      await page.getByRole('heading', { name: '使用统计', exact: true }).waitFor()
      await page.waitForFunction(theme => document.documentElement.dataset.theme === theme, theme)
      await page.evaluate(() => document.fonts.ready)
    }
    const capture = async (table, state, panel = false) => {
      if (!panel)
        await page.getByRole('button', { name: table.settings, exact: true }).scrollIntoViewIfNeeded()
      report.geometry.push({ table: table.id, state, ...await geometry(page, table, panel) })
      const filename = `${name}-${table.id}-${state}.png`
      await page.screenshot({ path: join(output, filename), fullPage: true, animations: 'disabled' })
      report.screenshots.push(filename)
    }
    await page.goto(new URL('/usage', base).href)
    await ready()
    for (const table of tables) {
      await selectTable(page, table)
      await checkKeyboard(page, table)
      const dialog = await openSettings(page, table)
      const checkbox = dialog.getByRole('checkbox', { name: table.hidden.label, exact: true })
      await checkbox.focus()
      await page.keyboard.press('Space')
      assert.equal(await checkbox.isChecked(), false)
      await assertColumns(page, table, true)
      await assertPreference(page, table, true)
      await capture(table, 'settings', true)
      await closeSettings(page, table)
    }

    await page.reload()
    await ready()
    for (const table of tables) {
      await selectTable(page, table, true)
      const dialog = await openSettings(page, table)
      assert.equal(await dialog.getByRole('checkbox', { name: table.hidden.label, exact: true }).isChecked(), false)
      await closeSettings(page, table)
      await capture(table, 'reloaded-hidden')
    }

    // Reset one table, reload, and verify the other table's overrides survive.
    for (const [index, table] of tables.entries()) {
      await selectTable(page, table, true)
      const dialog = await openSettings(page, table)
      await dialog.getByRole('button', { name: '恢复默认', exact: true }).click()
      await assertColumns(page, table)
      await assertPreference(page, table, false)
      assert.equal(await dialog.getByRole('checkbox', { name: table.hidden.label, exact: true }).isChecked(), true)
      await closeSettings(page, table)
      await capture(table, 'reset')
      await page.reload()
      await ready()
      for (const [otherIndex, other] of tables.entries()) {
        const hidden = otherIndex > index
        await selectTable(page, other, hidden)
        await assertPreference(page, other, hidden)
      }
    }
    for (const path of fixtures().keys())
      assert.ok(report.api.includes(path), `Expected synthetic API was not exercised: ${path}`)
    assert.deepEqual(report.blocked, [], 'Unexpected API, mutation, navigation or external network attempt')
    assert.deepEqual(report.pageErrors, [])
    assert.deepEqual(report.failedResources, [])
    report.result = 'passed'
    process.stdout.write(`Passed usage columns: ${name}\n`)
  }
  catch (error) {
    report.result = 'failed'
    report.error = error.stack ?? String(error)
    if (page)
      await page.screenshot({ path: join(output, `${name}-failure.png`), fullPage: true }).catch(() => {})
    throw error
  }
  finally {
    await writeFile(join(output, `${name}-report.json`), `${JSON.stringify(report, null, 2)}\n`)
      .finally(() => context.close())
  }
}

async function main() {
  const base = new URL(process.env.QA_BASE_URL || 'http://127.0.0.1:5198')
  assert.ok(['http:', 'https:'].includes(base.protocol) && ['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname), 'QA_BASE_URL must point to a loopback frontend, never production')
  assert.ok(!base.username && !base.password && base.pathname === '/' && !base.search && !base.hash, 'QA_BASE_URL must be a bare local origin')
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-usage-columns-qa'
  await mkdir(output, { recursive: true })
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const browser = await chromium.launch({
    headless: true,
    args: ['--disable-background-networking', '--no-proxy-server'],
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  })
  try {
    for (const theme of ['light', 'dark']) {
      for (const width of [1440, 390, 320])
        await runScenario(browser, base, output, theme, width)
    }
    process.stdout.write(`Passed: usage success/errors, persistence, independent reset, required columns, keyboard focus/Escape/Tab, light/dark at 1440/390/320px. Screenshots and reports: ${output}\n`)
  }
  finally {
    await browser.close()
  }
}

export { assertColumns, checkKeyboard, closeSettings, geometry, isolateNetwork, openSettings, runScenario, selectTable, tables }

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    process.stderr.write(`${error.stack ?? error}\n`)
    process.exitCode = 1
  })
}
