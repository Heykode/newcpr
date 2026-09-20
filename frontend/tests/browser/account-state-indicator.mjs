import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { accounts, reloginEntries } from '../fixtures/relogin-count-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-state-indicator-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ reducedMotion: 'reduce' })
  const errors = []
  const mutations = []
  let enabled = true
  let ready = true
  let modelCount = 2
  let errorReason = null
  page.on('pageerror', error => errors.push(error.message))
  const fulfill = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
  await page.route('**/dev/api/admin/auth/status', route => fulfill(route, { authenticated: true }))
  await page.route('**/dev/api/admin/system/version', route => fulfill(route, {
    version: '3.12.0',
    gitSha: 'qa',
    buildTime: new Date().toISOString(),
    deploymentMode: 'docker',
    deploymentModeLabel: 'Docker',
    updateChannel: 'stable',
    latestVersion: '3.12.0',
    hasUpdate: false,
    updateCached: true,
    updateWarning: null,
  }))
  await page.route('**/dev/api/admin/account-groups?*', route => fulfill(route, {
    items: [],
    page: { page: 1, pageSize: 200, total: 0, totalPages: 0 },
    configRevision: 1,
  }))
  await page.route('**/dev/api/admin/relogin', route => fulfill(route, {
    settings: { concurrency: 1, paused: false },
    items: reloginEntries,
  }))
  await page.route('**/dev/api/admin/relogin/accounts/query', route => fulfill(route, []))
  await page.route('**/dev/api/admin/accounts/import-tasks', route => fulfill(route, { items: [] }))
  await page.route('**/dev/api/admin/accounts?*', async (route) => {
    assert.equal(route.request().method(), 'GET')
    const expiresAt = minutes => new Date(Date.now() + minutes * 60_000).toISOString()
    const items = accounts.map((account, index) => ({
      ...account,
      ...(index === 0 && errorReason ? { status: 'error', errorReason } : {}),
      provider: index === 2 ? 'xai' : account.provider,
      turnStateInjectionEnabled: index === 0 ? enabled : index === 2,
      turnState: index === 0
        ? {
            enabled,
            requiredModels: ['model-a', 'model-b', ...Array.from({ length: modelCount - 2 }, (_, index) => `model-extra-${index}-with-a-long-name-for-overflow-checks`)],
            readyModels: ready ? [{ model: 'model-a', expiresAt: expiresAt(50) }] : [],
            models: ready
              ? [
                  {
                    model: 'model-a',
                    refreshStatus: 'cooldown',
                    probeAttempts: 11,
                    probeTotalAttempts: 456,
                    probeCooldownUntil: expiresAt(2),
                    probeRetryFromUpstream: true,
                    probeHttpStatus: 429,
                    probeErrorCode: 'rate_limit_exceeded',
                    probeReturnedLength: 356,
                    successfulProbeAttempt: 2,
                    active: { chars: 332, capturedAt: new Date(Date.now() - 10 * 60_000).toISOString(), expiresAt: expiresAt(50) },
                    standby: { chars: 332, expiresAt: expiresAt(55) },
                  },
                  {
                    model: 'model-b',
                    refreshStatus: 'refreshing',
                    probeAttempts: 121,
                    lastProbeReason: 'missing_state',
                    active: null,
                    standby: null,
                  },
                  ...Array.from({ length: modelCount - 2 }, (_, index) => ({
                    model: `model-extra-${index}-with-a-long-name-for-overflow-checks`,
                    refreshStatus: 'ready',
                    active: { chars: 332, expiresAt: expiresAt(40) },
                    standby: { chars: 332, expiresAt: expiresAt(45) },
                  })),
                ]
              : [
                  { model: 'model-a', refreshStatus: 'refreshing', active: null, standby: null },
                  { model: 'model-b', refreshStatus: 'refreshing', active: null, standby: null },
                ],
          }
        : null,
    }))
    await route.fulfill({ json: {
      code: 200,
      message: 'ok',
      data: {
        items,
        page: { page: 1, pageSize: 20, total: 3, totalPages: 1 },
        summary: { total: 3, normal: 3, error: 0, disabled: 0, rateLimited: 0, quotaExhausted: 0 },
      },
    } })
  })
  await page.route('**/dev/api/admin/accounts/batch-update', async (route) => {
    const body = route.request().postDataJSON()
    assert.deepEqual(body, { accountIds: [accounts[0].id], turnStateInjectionEnabled: !enabled })
    mutations.push(body)
    enabled = body.turnStateInjectionEnabled
    await route.fulfill({ json: { code: 200, message: 'ok', data: null } })
  })
  try {
    await page.setViewportSize({ width: 1440, height: 900 })
    await page.goto(`${process.env.QA_BASE_URL || 'http://127.0.0.1:5198'}/accounts`)
    const avatar = page.locator('[data-account-state-avatar]')
    const mark = page.locator('[data-account-state-mark]')
    await mark.waitFor()
    assert.equal(await mark.count(), 1)
    const identity = avatar.locator('..')
    await identity.locator('[data-account-totp-mark]').waitFor()
    assert.match(await mark.getAttribute('title'), /已就绪：model-a；待采集：model-b/)
    assert.equal(await page.locator('[data-account-state-ready]').count(), 1)
    assert.match(await avatar.getAttribute('class'), /ring-emerald-500/)
    assert.equal(await mark.getAttribute('title'), await mark.getAttribute('aria-label'))
    await page.locator('button[title="展开统计"]').first().click()
    const panel = page.locator('[data-account-turn-state-panel]')
    await panel.waitFor()
    for (const theme of ['light', 'dark']) {
      await page.setViewportSize({ width: 1440, height: 900 })
      if (await page.locator('html').getAttribute('data-theme') !== theme) {
        await page.getByRole('button', {
          name: theme === 'dark' ? '切换暗黑模式' : '切换浅色模式',
          exact: true,
        }).click()
      }
      await page.waitForFunction(value => document.documentElement.dataset.theme === value, theme)
      const rowSelection = page.getByRole('checkbox', { name: '选择账号', exact: true }).first()
      await rowSelection.press('Space')
      assert.equal(await rowSelection.isChecked(), true)
      const selectedRow = page.locator('tr[aria-selected="true"]').first()
      await selectedRow.hover()
      await page.waitForFunction(() => {
        const cell = document.querySelector('tr[aria-selected="true"] td')
        if (!cell)
          return false
        const probe = document.createElement('div')
        probe.style.backgroundColor = 'var(--cp-table-row-selected-bg)'
        cell.appendChild(probe)
        const expected = getComputedStyle(probe).backgroundColor
        probe.remove()
        return getComputedStyle(cell).backgroundColor === expected
      })
      await rowSelection.press('Space')
      assert.equal(await rowSelection.isChecked(), false)
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 900 })
        await avatar.evaluate(element => element.scrollIntoView({ block: 'center', inline: 'center' }))
        await panel.evaluate(element => element.scrollIntoView({ block: 'center', inline: 'nearest' }))
        const geometry = await identity.evaluate((element) => {
          const rect = node => ({
            x: node.getBoundingClientRect().x,
            y: node.getBoundingClientRect().y,
            width: node.getBoundingClientRect().width,
            height: node.getBoundingClientRect().height,
          })
          const avatar = element.querySelector('[data-account-state-avatar]')
          const currentRow = element.closest('tr')
          const other = [...document.querySelectorAll('tr[data-row-key]')]
            .find(row => row !== currentRow)
            ?.querySelector('[data-swipe-select-handle]')
          return {
            avatar: rect(avatar),
            other: rect(other),
            state: rect(element.querySelector('[data-account-state-mark]')),
            totp: rect(element.querySelector('[data-account-totp-mark]')),
            background: getComputedStyle(avatar).backgroundColor,
            ordinaryBackground: getComputedStyle(other).backgroundColor,
            ring: getComputedStyle(avatar).boxShadow,
          }
        })
        assert.equal(geometry.avatar.width, 36)
        assert.equal(geometry.avatar.height, 36)
        assert.equal(geometry.other.width, geometry.avatar.width)
        assert.equal(geometry.state.width, 16)
        assert.equal(geometry.totp.width, 16)
        assert.ok(geometry.state.x > geometry.totp.x + geometry.totp.width)
        assert.ok(geometry.state.y > geometry.totp.y + geometry.totp.height)
        assert.notEqual(geometry.background, geometry.ordinaryBackground)
        assert.match(geometry.ring, /inset/)
        const statePanel = await panel.evaluate(element => ({
          width: element.getBoundingClientRect().width,
          clientWidth: element.clientWidth,
          scrollWidth: element.scrollWidth,
          rowCount: element.querySelectorAll('[data-account-turn-state-model]').length,
          rowOverflow: [...element.querySelectorAll('[data-account-turn-state-model]')]
            .some(row => row.scrollWidth > row.clientWidth),
          text: element.textContent,
        }))
        assert.equal(statePanel.rowCount, 2)
        assert.ok(statePanel.scrollWidth <= statePanel.clientWidth)
        assert.equal(statePanel.rowOverflow, false)
        assert.match(statePanel.text, /1\/2 模型/)
        assert.match(statePanel.text, /2 个 State/)
        assert.match(statePanel.text, /可用 · 探测冷却/)
        assert.match(statePanel.text, /累计 456 次/)
        assert.match(statePanel.text, /s 后重试/)
        assert.equal((statePanel.text.match(/332 字符/g) ?? []).length, 2)
        if (width < 640)
          assert.ok(statePanel.width <= width - 80, statePanel)
        await page.screenshot({ path: `${output}/${theme}-${width}.png`, fullPage: true })
      }
      for (const count of [3, 4, 10]) {
        modelCount = count
        await page.reload()
        await page.locator('button[title="展开统计"]').first().click()
        await panel.waitFor()
        for (const width of [1440, 390, 320]) {
          await page.setViewportSize({ width, height: 900 })
          await panel.evaluate(element => element.scrollIntoView({ block: 'center', inline: 'nearest' }))
          const list = panel.locator('[data-account-turn-state-list]')
          await list.evaluate(element => element.scrollTo({ top: 0, behavior: 'instant' }))
          const metrics = await list.evaluate((element) => {
            const rows = [...element.querySelectorAll('[data-account-turn-state-model]')]
            return {
              count: rows.length,
              height: element.clientHeight,
              contentHeight: element.scrollHeight,
              rowHeights: rows.map(row => row.getBoundingClientRect().height),
              overflow: rows.some(row => row.scrollWidth > row.clientWidth),
              panelHeight: element.parentElement.getBoundingClientRect().height,
              headerTop: element.parentElement.firstElementChild.getBoundingClientRect().top,
            }
          })
          const rowHeight = width < 640 ? 96 : 80
          assert.equal(metrics.count, count)
          assert.ok(metrics.rowHeights.every(height => height === rowHeight), JSON.stringify(metrics))
          assert.equal(metrics.height, 3 * rowHeight)
          assert.equal(metrics.overflow, false)
          assert.equal(metrics.contentHeight > metrics.height, count > 3)
          if (count > 3) {
            await list.focus()
            for (let step = 0; step < count; step++) {
              await list.press('PageDown')
            }
            await page.waitForFunction(() => {
              const element = document.querySelector('[data-account-turn-state-list]')
              return element.scrollTop + element.clientHeight >= element.scrollHeight - 1
            })
            const after = await list.evaluate(element => ({
              panelHeight: element.parentElement.getBoundingClientRect().height,
              headerTop: element.parentElement.firstElementChild.getBoundingClientRect().top,
            }))
            assert.equal(after.panelHeight, metrics.panelHeight)
            assert.equal(after.headerTop, metrics.headerTop)
          }
          await list.evaluate(element => element.scrollTo({ top: 0, behavior: 'instant' }))
          if (count === 4)
            await page.screenshot({ path: `${output}/${theme}-${width}-four-models.png`, fullPage: true })
        }
      }
      modelCount = 2
      await page.reload()
      await page.locator('button[title="展开统计"]').first().click()
      await panel.waitFor()
    }
    for (const [reason, label] of [
      ['credential_expired', '凭据已失效，需要重新登录'],
      ['access_token_expired', '凭据已过期，等待自动刷新'],
    ]) {
      errorReason = reason
      await page.reload()
      await mark.waitFor()
      assert.equal(await page.locator('[data-account-state-ready]').count(), 0)
      assert.match(await avatar.getAttribute('class'), /ring-cp-error/)
      assert.equal(await mark.getAttribute('title'), `State：${label}`)
      await page.locator('button[title="展开统计"]').first().click()
      await panel.locator('[data-account-turn-state-blocked]').waitFor()
      assert.equal(await panel.locator('[data-account-turn-state-model]').count(), 2)
      assert.ok((await panel.textContent()).includes('0/2 模型'))
      assert.ok((await panel.textContent()).includes(label))
      for (const width of [1440, 320]) {
        await page.setViewportSize({ width, height: 900 })
        await panel.evaluate(element => element.scrollIntoView({ block: 'center', inline: 'nearest' }))
        assert.ok(await panel.evaluate(element => element.scrollWidth <= element.clientWidth))
        await page.screenshot({ path: `${output}/${reason}-${width}.png`, fullPage: true })
      }
    }
    errorReason = null
    await page.reload()
    await mark.waitFor()
    assert.equal(await page.locator('[data-account-state-ready]').count(), 1)
    assert.match(await avatar.getAttribute('class'), /ring-emerald-500/)
    ready = false
    await page.reload()
    await mark.waitFor()
    assert.equal(await page.locator('[data-account-state-ready]').count(), 0)
    assert.match(await avatar.getAttribute('class'), /ring-amber-500/)
    assert.match(await mark.getAttribute('title'), /待采集/)
    await page.setViewportSize({ width: 1440, height: 900 })
    const more = page.getByRole('button', { name: '更多操作', exact: true }).first()
    for (const next of [false, true]) {
      await more.click()
      await page.getByRole('button', { name: next ? '开启 State 注入' : '关闭 State 注入', exact: true }).click()
      await page.waitForFunction(value =>
        document.querySelectorAll('[data-account-state-mark]').length === (value ? 1 : 0), next)
    }
    assert.equal(mutations.length, 2)
    assert.deepEqual(errors, [])
    process.stdout.write('Passed: State readiness panel, credential error/recovery, 2/3/4/10 models, bounded scrolling and keyboard access, slot lengths/countdowns, green/gold/error avatar, menu toggle, partial update, 2FA coexistence, provider guard, light/dark and 1440/390/320px.\n')
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
