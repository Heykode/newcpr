import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { accounts } from '../fixtures/relogin-count-data.mjs'

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
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/dev/api/admin/accounts?*', async (route) => {
    assert.equal(route.request().method(), 'GET')
    const items = accounts.map((account, index) => ({
      ...account,
      provider: index === 2 ? 'xai' : account.provider,
      turnStateInjectionEnabled: index === 0 ? enabled : index === 2,
      turnState: index === 0 && enabled
        ? {
            requiredModels: ['model-a', 'model-b'],
            readyModels: ready ? [{ model: 'model-a', expiresAt: new Date(Date.now() + 3600000).toISOString() }] : [],
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
    for (const theme of ['light', 'dark']) {
      await page.setViewportSize({ width: 1440, height: 900 })
      if (await page.locator('html').getAttribute('data-theme') !== theme) {
        await page.getByRole('button', {
          name: theme === 'dark' ? '切换暗黑模式' : '切换浅色模式',
          exact: true,
        }).click()
      }
      await page.waitForFunction(value => document.documentElement.dataset.theme === value, theme)
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 900 })
        await avatar.evaluate(element => element.scrollIntoView({ block: 'center', inline: 'center' }))
        const geometry = await identity.evaluate((element) => {
          const rect = node => ({
            x: node.getBoundingClientRect().x,
            y: node.getBoundingClientRect().y,
            width: node.getBoundingClientRect().width,
            height: node.getBoundingClientRect().height,
          })
          const avatar = element.querySelector('[data-account-state-avatar]')
          const other = element.closest('tr').nextElementSibling.querySelector('[data-swipe-select-handle]')
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
        await page.screenshot({ path: `${output}/${theme}-${width}.png`, fullPage: true })
      }
    }
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
    process.stdout.write('Passed: State readiness, green/gold avatar, menu toggle, partial update, 2FA coexistence, provider guard, light/dark and 1440/390/320px.\n')
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
