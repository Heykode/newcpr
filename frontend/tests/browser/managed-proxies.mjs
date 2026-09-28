import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH || undefined })
  const base = process.env.QA_BASE_URL || 'http://127.0.0.1:5217'
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-mihomo-ui'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
  const errors = []
  const commands = []
  let pendingFilter
  let batchFixture = false
  let checkGate
  let activeChecks = 0
  let peakChecks = 0
  const checkRequests = []
  page.on('pageerror', error => errors.push(error.message))
  const warm = { ready: 3, target: 5, eligible: 5, checking: 1, cooling: 1, readySubscription: 2, readyDynamic: 1, failureReasons: { stream_failure: 1 } }
  const state = {
    version: 'v1.19.31',
    installed: true,
    supported: true,
    running: true,
    busy: false,
    phase: 'idle',
    error: null,
    endpoint: 'http://127.0.0.1:3101',
    subscriptionDownloadMode: 'auto',
    dynamicProxies: 1,
    countryFilter: { mode: 'exclude', codes: ['CN', 'HK'], allowUnknown: false, dynamicProviderManaged: true },
    countryCodes: ['US', 'GB', 'JP', 'SG', 'DE', 'HK', 'CN'],
    bpsWarmPool: warm,
    bpsIpWarmPool: warm,
    codexWarmPool: warm,
    codexIpWarmPool: warm,
    subscriptionItems: [{ id: 'fixture-source', label: '测试订阅', enabled: true, nodes: 32, cached: true, updatedAt: '2026-09-28T00:00:00Z' }],
    nodeStates: [
      { name: 'node-fixture', displayName: '东京 01', subscriptionIds: ['fixture-source'], dynamic: false, state: 'ready', countryCode: 'JP', countryBlocked: false, check: null },
      { name: 'DYNAMIC-fixture', displayName: '动态代理 01', subscriptionIds: [], dynamic: true, state: 'ready', countryCode: null, countryBlocked: false, check: null },
      { name: 'sg-known', displayName: '新加坡 已确认', subscriptionIds: ['fixture-source'], dynamic: false, state: 'ready', countryCode: 'SG', countryCheckedAt: '2026-09-28T00:00:00Z', countryError: null, countryBlocked: false, check: null },
      { name: 'sg-unknown', displayName: '新加坡 未检测', subscriptionIds: ['fixture-source'], dynamic: false, state: 'ready', countryCode: null, countryCheckedAt: null, countryError: null, countryBlocked: true, check: { exitIp: '203.0.113.10', countryCode: 'SG', countryName: 'Singapore', city: 'Singapore' } },
      { name: 'sg-failed', displayName: '新加坡 检测失败', subscriptionIds: ['fixture-source'], dynamic: false, state: 'ready', countryCode: null, countryCheckedAt: '2026-09-28T00:00:00Z', countryError: 'lookup_failed', countryBlocked: true, check: null },
      { name: 'cn-excluded', displayName: '中国 排除', subscriptionIds: ['fixture-source'], dynamic: false, state: 'ready', countryCode: 'CN', countryBlocked: true, check: null },
    ],
  }
  const check = { checkedAt: '2026-09-28T00:00:00Z', success: true, latencyMs: 35, message: '代理出口连通正常', exitIp: '203.0.113.9', countryCode: 'JP', quality: { checkedAt: '2026-09-28T00:00:00Z', score: 100, grade: 'A', summary: '通过 1 项，告警 0 项', checks: [{ name: 'openai', status: 'pass', success: true, httpStatus: 401, latencyMs: 35, reason: 'HTTP 401（目标可达）' }] } }
  await page.route('**/api/admin/**', async (route) => {
    const request = route.request()
    const path = new URL(request.url()).pathname.replace(/^\/dev/, '')
    let data = null
    if (path.endsWith('/auth/status')) {
      data = { authenticated: true }
    }
    else if (path.endsWith('/system/version')) {
      data = { version: 'isolated-mihomo-ui', buildType: 'test' }
    }
    else if (path.endsWith('/proxies/mihomo/check')) {
      if (batchFixture) {
        const payload = request.postDataJSON()
        checkRequests.push(payload)
        activeChecks++
        peakChecks = Math.max(peakChecks, activeChecks)
        await (checkGate ?? new Promise(resolve => setTimeout(resolve, 40)))
        activeChecks--
        if (payload.node === 'batch-006') {
          await route.fulfill({ status: 500, json: { code: 500, message: 'synthetic node failure', data: null } })
          return
        }
        const status = payload.node === 'batch-003' ? 'fail' : payload.node === 'batch-004' ? 'warn' : payload.node === 'batch-005' ? 'challenge' : 'pass'
        data = { ...check, success: payload.node !== 'batch-003', quality: payload.quality ? { ...check.quality, checks: [{ ...check.quality.checks[0], status }] } : null }
      }
      else {
        data = check
      }
    }
    else if (path.endsWith('/proxies/mihomo')) {
      if (request.method() === 'POST') {
        const command = request.postDataJSON()
        commands.push(command)
        if (command.action === 'country_filter') {
          pendingFilter = command.countryFilter
          state.busy = true
          state.phase = 'CountryFilter'
        }
      }
      data = state
    }
    else if (path.endsWith('/proxies')) {
      data = { items: [], page: { page: 1, pageSize: 20, total: 0, totalPages: 0 } }
    }
    await route.fulfill({ json: { code: 200, message: 'ok', data } })
  })
  try {
    await page.goto(`${base}/proxies`)
    await page.getByRole('button', { name: '订阅管理', exact: true }).click()
    await page.getByText('测试订阅', { exact: true }).waitFor()
    await page.getByRole('textbox', { name: '订阅地址（每行一个）', exact: true }).fill('https://fixture.invalid/subscription')
    await page.getByRole('button', { name: '添加订阅', exact: true }).click()
    await page.waitForFunction(() => document.querySelector('textarea')?.value === '')
    assert.equal(commands.at(-1).action, 'subscription_add')
    await page.getByRole('textbox', { name: '订阅地址（每行一个）', exact: true }).fill('do-not-copy-between-tabs')
    await page.getByRole('button', { name: '动态代理', exact: true }).click()
    assert.equal(await page.getByRole('textbox', { name: '动态代理（每行一个）', exact: true }).inputValue(), '')
    await page.getByRole('textbox', { name: '动态代理（每行一个）', exact: true }).fill('proxy.example:8000:user:password')
    await page.getByRole('button', { name: '追加', exact: true }).click()
    await page.waitForFunction(() => document.querySelector('textarea')?.value === '')
    assert.equal(commands.at(-1).action, 'dynamic_append')
    await page.getByRole('button', { name: '节点管理', exact: true }).click()
    await page.getByText('东京 01', { exact: true }).waitFor()
    const unknownRow = page.locator('tr').filter({ has: page.getByText('新加坡 未检测', { exact: true }) })
    await unknownRow.getByText('地区未检测，暂不准入', { exact: true }).waitFor()
    await unknownRow.getByText('规则地区：未知', { exact: true }).waitFor()
    assert.match(await unknownRow.textContent(), /连接诊断：Singapore/)
    assert.equal(await unknownRow.getByText('地区已排除', { exact: true }).count(), 0)
    const knownRow = page.locator('tr').filter({ has: page.getByText('新加坡 已确认', { exact: true }) })
    assert.equal(await knownRow.locator('[data-country-block-reason]').count(), 0)
    await page.getByText('地区检测失败，暂不准入', { exact: true }).waitFor()
    await page.getByText('已排除地区：中国 CN', { exact: true }).waitFor()
    await page.getByRole('button', { name: '完整质量检测', exact: true }).first().click()
    await page.getByRole('dialog').getByText('连通检测通过 · A · 100 分', { exact: true }).waitFor()
    await page.getByRole('dialog').getByText('HTTP 401（目标可达）', { exact: true }).waitFor()
    await page.getByRole('dialog').getByText(/不是模型调用成功/).waitFor()
    await page.keyboard.press('Escape')
    await page.getByRole('dialog').waitFor({ state: 'hidden' })
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: width < 500 ? 844 : 1000 })
      for (const [tab, filename] of [['节点管理', 'nodes'], ['订阅管理', 'sources'], ['内核与规则', 'kernel']]) {
        await page.getByRole('button', { name: tab, exact: true }).click()
        await page.getByText('Mihomo v1.19.31', { exact: true }).waitFor()
        if (filename === 'kernel') {
          assert.equal(await page.getByText('允许未知地区', { exact: true }).isVisible(), true)
          assert.equal(await page.getByText('动态代理地区由供应商管理', { exact: true }).isVisible(), true)
          assert.equal(await page.getByRole('checkbox', { name: '美国 US', exact: true }).count(), 1)
          assert.match(await page.locator('[data-country-progress]').textContent(), /已确认 3 \/ 5 · 未检测 1 · 检测失败 1/)
        }
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), `${tab} overflow at ${width}`)
        await page.screenshot({ path: `${output}/${filename}-${width}.png`, fullPage: true })
      }
    }
    await page.setViewportSize({ width: 1440, height: 1000 })
    const countrySearch = page.getByRole('textbox', { name: '搜索地区代码', exact: true })
    for (const query of ['新加坡', 'Singapore', 'sg']) {
      await countrySearch.fill(query)
      await page.getByRole('checkbox', { name: '新加坡 SG', exact: true }).waitFor()
      assert.equal(await page.getByRole('checkbox', { name: '美国 US', exact: true }).count(), 0)
    }
    const singapore = page.getByRole('checkbox', { name: '新加坡 SG', exact: true })
    await singapore.locator('..').click()
    assert.equal(await singapore.isChecked(), true)
    await page.getByRole('button', { name: '保存地区规则', exact: true }).click()
    await page.getByText('地区规则等待确认', { exact: true }).waitFor()
    assert.equal(commands.at(-1).action, 'country_filter')
    assert.deepEqual(pendingFilter.codes, ['CN', 'HK', 'SG'])
    const refresh = page.getByRole('button', { name: '刷新受管代理状态', exact: true })
    await refresh.click()
    assert.equal(await singapore.isChecked(), true)
    assert.equal(await singapore.isDisabled(), true)
    state.countryFilter = pendingFilter
    state.busy = false
    state.phase = 'completed'
    await refresh.click()
    await page.getByText('地区规则等待确认', { exact: true }).waitFor({ state: 'hidden' })
    assert.equal(await singapore.isChecked(), true)
    assert.equal(await singapore.isEnabled(), true)
    await countrySearch.fill('JP')
    const japan = page.getByRole('checkbox', { name: '日本 JP', exact: true })
    await japan.locator('..').click()
    await refresh.click()
    assert.equal(await japan.isChecked(), true)
    await page.getByText('地区规则草稿未保存', { exact: true }).waitFor()
    await page.getByRole('button', { name: '保存地区规则', exact: true }).click()
    await page.getByText('地区规则等待确认', { exact: true }).waitFor()
    state.busy = false
    state.phase = 'failed'
    state.error = '合成测试：地区规则发布失败'
    await refresh.click()
    await page.getByRole('alert').getByText('合成测试：地区规则发布失败', { exact: true }).waitFor()
    assert.equal(await japan.isChecked(), true)
    await page.getByText('地区规则草稿未保存', { exact: true }).waitFor()
    state.error = null
    state.nodeStates = Array.from({ length: 106 }, (_, index) => {
      const id = index + 1
      const name = `batch-${String(id).padStart(3, '0')}`
      return {
        ...state.nodeStates[0],
        name,
        displayName: name,
        dynamic: id === 106,
        subscriptionIds: id === 106 ? [] : id === 1 ? ['source-a', 'source-b'] : id <= 60 ? ['source-a'] : ['source-b'],
        check: null,
      }
    })
    state.subscriptionItems = ['a', 'b'].map(id => ({ ...state.subscriptionItems[0], id: `source-${id}`, label: `订阅 ${id.toUpperCase()}` }))
    batchFixture = true
    const allNodes = [...state.nodeStates]
    await page.getByRole('button', { name: '节点管理', exact: true }).click()
    await page.getByRole('checkbox', { name: '选择节点 batch-001', exact: true }).waitFor()
    const rows = page.locator('tbody tr')
    assert.equal(await rows.count(), 50)
    assert.match(await page.locator('[data-node-selection]').textContent(), /全部筛选结果 106 个/)
    const toggle = name => page.getByRole('checkbox', { name, exact: true }).locator('..').click()
    const chooseSource = async (name) => {
      await page.getByRole('combobox', { name: '节点来源', exact: true }).click()
      await page.getByRole('option', { name, exact: true }).click()
    }
    const waitBatch = async (kind, count) => {
      await page.locator('[data-node-batch]').getByText(new RegExp(`${kind}完成 ${count}/${count}`)).waitFor()
      assert.equal(await page.getByRole('dialog').count(), 0)
    }
    await toggle('选择节点 batch-001')
    await page.getByRole('button', { name: '下一页', exact: true }).click()
    await toggle('选择节点 batch-051')
    assert.match(await page.locator('[data-node-selection]').textContent(), /已选 2 · 本次检测 2/)
    await page.getByRole('button', { name: '批量测试连接', exact: true }).click()
    await waitBatch('连接检测', 2)
    assert.deepEqual(checkRequests.map(item => item.node).sort(), ['batch-001', 'batch-051'])
    assert.equal(peakChecks, 2)
    // Changing filters resets both the page and selections, including cross-page selections.
    await chooseSource('订阅 B')
    assert.equal(await rows.count(), 46)
    assert.equal(await page.getByRole('checkbox', { name: '选择节点 batch-001', exact: true }).isChecked(), false)
    assert.match(await page.locator('[data-node-selection]').textContent(), /未勾选.*46 个/)
    checkRequests.length = 0
    peakChecks = 0
    await page.getByRole('button', { name: '批量测试连接', exact: true }).click()
    await waitBatch('连接检测', 46)
    assert.equal(peakChecks, 3)
    assert.equal(checkRequests.length, 46)
    assert.ok(checkRequests.every(item => item.node === 'batch-001' || Number(item.node.slice(-3)) > 60))
    assert.ok(checkRequests.every(item => !item.quality && item.node !== 'batch-106'))
    await chooseSource('动态代理')
    assert.equal(await rows.count(), 1)
    await page.getByRole('checkbox', { name: '选择节点 batch-106', exact: true }).waitFor()
    await chooseSource('全部订阅节点')
    assert.match(await page.locator('[data-node-selection]').textContent(), /105 个/)
    await chooseSource('所有来源')
    checkRequests.length = 0
    peakChecks = 0
    await page.getByRole('button', { name: '批量测试连接', exact: true }).click()
    await waitBatch('连接检测', 106)
    assert.equal(peakChecks, 3)
    assert.equal(new Set(checkRequests.map(item => item.node)).size, 106)
    assert.match(await page.locator('[data-node-batch]').textContent(), /通过 104 · 失败 2/)
    // Page checkbox selects 50, never every filtered node; scope stays frozen while running.
    await toggle('选择本页全部节点')
    checkRequests.length = 0
    peakChecks = 0
    await page.getByRole('button', { name: '批量质量检测', exact: true }).click()
    const search = page.getByRole('textbox', { name: '搜索受管节点', exact: true })
    await search.fill('batch-106')
    await waitBatch('质量检测', 50)
    assert.equal(peakChecks, 2)
    assert.equal(new Set(checkRequests.map(item => item.node)).size, 50)
    assert.ok(checkRequests.every(item => item.quality && Number(item.node.slice(-3)) <= 50))
    assert.match(await page.locator('[data-node-batch]').textContent(), /通过 46 · 失败 2 · 告警 1 · 挑战 1/)
    assert.match(await page.locator('[data-node-selection]').textContent(), /未勾选.*1 个/)
    await search.fill('')
    // Refresh may remove whole pages without a filter change; clamp the actual page.
    await page.getByRole('button', { name: '下一页', exact: true }).click()
    await page.getByRole('button', { name: '下一页', exact: true }).click()
    assert.equal(await rows.count(), 6)
    state.nodeStates = allNodes.slice(0, 20)
    await page.getByRole('button', { name: '刷新受管代理状态', exact: true }).click()
    await page.getByRole('checkbox', { name: '选择节点 batch-001', exact: true }).waitFor()
    assert.equal(await rows.count(), 20)
    assert.equal(await page.getByRole('button', { name: '上一页', exact: true }).isDisabled(), true)
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: width < 500 ? 844 : 1000 })
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), `batch overflow at ${width}`)
      await page.screenshot({ path: `${output}/batch-${width}.png`, fullPage: true })
    }
    // A departed management panel must not continue dispatching the rest of the batch.
    checkRequests.length = 0
    let releaseChecks
    checkGate = new Promise((resolve) => {
      releaseChecks = resolve
    })
    await page.getByRole('button', { name: '批量测试连接', exact: true }).click()
    await page.locator('tbody tr').first().getByText('检测中', { exact: true }).waitFor()
    await page.getByRole('button', { name: '订阅管理', exact: true }).click()
    await page.getByRole('textbox', { name: '订阅地址（每行一个）', exact: true }).waitFor()
    releaseChecks()
    await page.waitForTimeout(200)
    assert.equal(checkRequests.length, 3, `unmounted panel dispatched ${checkRequests.length} nodes`)
    assert.equal(activeChecks, 0)
    assert.ok(commands.every(command => !['probe', 'disable', 'recover'].includes(command.action)))
    assert.deepEqual(errors, [])
    process.stdout.write(`${JSON.stringify({ commands: commands.map(c => c.action), screenshots: 12, batchFixtureNodes: 106, connectionConcurrency: 3, qualityConcurrency: 2, errors })}\n`)
  }
  finally { await browser.close() }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
