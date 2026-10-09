import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'
import { mobileAccounts } from '../fixtures/mobile-account-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-mobile-accounts'
  await mkdir(output, { recursive: true })
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    server: { host: '127.0.0.1', port: 0 },
    plugins: [{ name: 'mobile-cards-isolation', configResolved(config) { config.server.proxy = {} } }],
  })
  let browser
  try {
    await server.listen()
    const base = `http://127.0.0.1:${server.httpServer.address().port}`
    browser = await chromium.launch({ headless: true })
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, reducedMotion: 'reduce' })
    page.setDefaultTimeout(15000)
    const rows = structuredClone(mobileAccounts)
    const errors = []
    const unexpected = []
    const reads = []
    const writes = []
    let empty = false
    page.on('pageerror', error => errors.push(error.message))
    const fulfill = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
    await page.route('**/*', (route) => {
      const request = route.request()
      const url = new URL(request.url())
      if (url.origin !== base) {
        unexpected.push('external request')
        return route.abort()
      }
      if (!url.pathname.startsWith('/dev/api/'))
        return route.continue()
      const path = url.pathname.replace('/dev', '')
      if (path === '/api/admin/accounts' && request.method() === 'GET') {
        reads.push(Object.fromEntries(url.searchParams))
        const current = Number(url.searchParams.get('page') || 1)
        const search = url.searchParams.get('search') || ''
        const matches = empty ? [] : rows.filter(row => !search || row.email.includes(search) || row.customName?.includes(search))
        return fulfill(route, {
          items: matches.slice((current - 1) * 3, current * 3),
          page: { page: current, pageSize: 3, total: matches.length, totalPages: Math.ceil(matches.length / 3) },
          summary: { total: matches.length, normal: 3, error: 1, rateLimited: 1, disabled: 1, quotaExhausted: 0 },
        })
      }
      if (path === '/api/admin/accounts/update' && request.method() === 'POST') {
        const body = request.postDataJSON()
        writes.push(body)
        Object.assign(rows.find(row => row.id === body.accountId), body)
        return fulfill(route, { accountId: body.accountId, configRevision: writes.length })
      }
      if (path === '/api/admin/accounts/quota/refresh' && request.method() === 'POST') {
        const body = request.postDataJSON()
        writes.push(body)
        const row = rows.find(row => row.id === body.accountId)
        row.quota.refreshedAtDisplay = 'mobile-refresh-complete'
        return fulfill(route, { account: row })
      }
      if (path === '/api/admin/auth/refresh')
        return fulfill(route, { authenticated: true })
      if (['/api/admin/relogin/accounts/query', '/api/admin/accounts/reset-credits/cache'].includes(path))
        return fulfill(route, [])
      if (request.method() !== 'GET') {
        unexpected.push(`${request.method()} ${path}`)
        return route.abort()
      }
      switch (path) {
        case '/api/admin/auth/status': return fulfill(route, { authenticated: true })
        case '/api/admin/system/version': return fulfill(route, { version: 'mobile-fixture', buildType: 'test' })
        case '/api/admin/account-groups':
        case '/api/admin/proxies': return fulfill(route, { items: [], page: { page: 1, pageSize: 200, total: 0, totalPages: 0 } })
        case '/api/admin/account-templates':
        case '/api/admin/accounts/import-tasks': return fulfill(route, { items: [] })
        case '/api/admin/accounts/reset-credits/batches': return fulfill(route, [])
        case '/api/admin/relogin': return fulfill(route, {
          items: [{ email: rows[0].email, hasTotp: true }],
          settings: { concurrency: 1, paused: false },
        })
        case '/api/admin/ipv6-egress': return fulfill(route, { revision: 1, defaultMode: 'unchanged', addresses: [], accountOverrides: {}, fixedBindings: {} })
        default:
          unexpected.push(path)
          return route.abort()
      }
    })
    const first = () => page.locator(`[data-account-card="${rows[0].id}"]`)
    const check = row => row.getByRole('checkbox', { name: '选择账号', exact: true }).locator('..').click()
    const selectAll = () => page.getByRole('checkbox', { name: '选择当前页账号', exact: true }).locator('..').click()
    const chooseSort = async (name) => {
      const response = page.waitForResponse(result => result.url().includes('/api/admin/accounts?') && result.url().includes('sortBy='))
      await page.getByRole('combobox', { name: '账号排序', exact: true }).click()
      await page.getByRole('option', { name, exact: true }).click()
      await response
    }
    try {
      await page.goto(`${base}/accounts`)
      await first().waitFor()
      assert.equal(await page.locator('.account-table').count(), 0)
      assert.equal(reads.length, 1, 'one initial list fetch')
      await first().getByText(rows[0].customName, { exact: true }).waitFor()
      await first().getByText(rows[0].email, { exact: true }).waitFor()
      await first().locator('[data-account-totp-mark]').waitFor()
      await first().locator('[data-account-excel-mark]').waitFor()
      await first().getByRole('link', { name: /监测/ }).waitFor()
      assert.equal(await first().getByRole('progressbar').count(), 2)
      assert.equal(await first().getByRole('checkbox').isChecked(), false)

      for (const theme of ['light', 'dark']) {
        await page.setViewportSize({ width: 1440, height: 1000 })
        if (await page.locator('html').getAttribute('data-theme') !== theme) {
          await page.getByRole('button', { name: theme === 'dark' ? '切换暗黑模式' : '切换浅色模式', exact: true }).evaluate(button => button.click())
          await page.waitForFunction(value => document.documentElement.dataset.theme === value, theme)
        }
        for (const width of [320, 390, 430, 767]) {
          await page.setViewportSize({ width, height: 900 })
          await first().scrollIntoViewIfNeeded()
          assert.ok(await page.locator('[data-account-mobile-list]').evaluate(el => el.scrollWidth <= el.clientWidth + 1), `list overflow ${width}`)
          assert.ok(await page.locator('[data-account-card]').evaluateAll(cards => cards.every((card) => {
            const box = card.getBoundingClientRect()
            if (box.left < 0 || box.right > window.innerWidth || card.scrollWidth > card.clientWidth + 1)
              return false
            return [...card.querySelectorAll('button, [role="progressbar"], [role="link"]')].every((el) => {
              const rect = el.getBoundingClientRect()
              return rect.left >= box.left && rect.right <= box.right + 1
            })
          })), `card bounds ${width}`)
          const title = await first().getByText(rows[0].customName, { exact: true }).boundingBox()
          const edit = await first().getByRole('button', { name: '编辑账号', exact: true }).boundingBox()
          assert.ok(title.x + title.width <= edit.x, 'title and controls do not overlap')
          const metadata = first().locator('[data-account-mobile-meta]')
          assert.equal(await metadata.getByRole('link', { name: /监测/ }).count(), 1)
          assert.equal(await metadata.locator('[data-account-plan-mark]').count(), 1)
          const usageHeader = first().locator('[data-account-usage-header]')
          const health = usageHeader.getByRole('group', { name: '最近 30 分钟账号健康状态', exact: true })
          assert.equal(await health.getByRole('button').count(), 6)
          const healthBox = await health.boundingBox()
          const tokensBox = await usageHeader.getByText('Tokens', { exact: true }).boundingBox()
          const typeBox = await usageHeader.getByText('OAuth', { exact: true }).boundingBox()
          assert.ok(healthBox.x >= tokensBox.x + tokensBox.width && healthBox.x + healthBox.width <= typeBox.x, 'health fits between tokens and authentication type')
          assert.ok((await usageHeader.boundingBox()).height <= 14.5, 'health never adds a row or increases header height')
          const footer = first().locator('[data-account-mobile-footer]')
          const capacity = await footer.getByText('容量', { exact: true }).boundingBox()
          const groups = await footer.getByRole('button', { name: /^查看账号分组/ }).boundingBox()
          assert.ok(groups.x > capacity.x + capacity.width, 'groups share the footer with capacity')
          const billing = first().locator('[data-account-inline-billing]')
          const credits = await billing.getByLabel('Codex 点数', { exact: true }).boundingBox()
          const spending = await billing.getByText('消费：$12.50', { exact: true }).boundingBox()
          const forecast = await billing.getByRole('button', { name: /^预计周额度/ }).boundingBox()
          if (width >= 390) {
            assert.ok(Math.abs(credits.y - spending.y) <= 4 && Math.abs(credits.y - forecast.y) <= 4, 'credit, spending and forecast share a line when space permits')
            assert.ok((await first().boundingBox()).height <= 310, 'normal mobile card remains compact')
            if (width === 390)
              assert.ok((await first().boundingBox()).height <= 267, 'health preserves the existing 266px example card')
          }
          await page.screenshot({ path: `${output}/mobile-${theme}-${width}.png` })
          await first().screenshot({ path: `${output}/card-${theme}-${width}.png` })
        }
      }
      await page.setViewportSize({ width: 390, height: 844 })
      const healthButtons = first().locator('[data-account-mobile-health]').getByRole('button')
      const readsBeforeHealth = reads.length
      for (const [index, expected] of [[0, '100%'], [1, '90%'], [2, '60%'], [3, '暂无样本']]) {
        await healthButtons.nth(index).click()
        const healthDetails = page.locator('section').filter({ has: page.locator('dt').getByText('已结束请求数', { exact: true }) })
        await healthDetails.waitFor()
        assert.equal((await healthDetails.locator('dt').getByText('成功率', { exact: true }).locator('+ dd').textContent()).trim(), expected)
        await page.keyboard.press('Escape')
        await healthDetails.waitFor({ state: 'hidden' })
      }
      assert.equal(await first().getByRole('checkbox').isChecked(), false, 'health clicks never select the card')
      assert.equal(reads.length, readsBeforeHealth, 'health details use existing snapshots without refetching')
      assert.equal(writes.length, 0, 'health details never mutate account state')
      await check(first())
      await selectAll()
      assert.equal(await page.locator('[data-account-card][data-selected]').count(), 3)
      await page.getByRole('button', { name: '下一页', exact: true }).click()
      await page.locator(`[data-account-card="${rows[3].id}"]`).waitFor()
      await check(page.locator(`[data-account-card="${rows[3].id}"]`))
      await page.getByText('已选 4', { exact: true }).waitFor()
      await page.getByRole('button', { name: '批量编辑账号', exact: true }).click()
      const batchDialog = page.getByRole('dialog')
      await batchDialog.getByText('已选择 4 个账号', { exact: true }).waitFor()
      await batchDialog.getByRole('button', { name: '取消', exact: true }).click()
      await batchDialog.waitFor({ state: 'hidden' })
      assert.equal(writes.length, 0, 'cancelling batch editing never writes')
      const unknown = page.locator(`[data-account-card="${rows[4].id}"]`)
      assert.equal(await unknown.getByRole('progressbar').first().getAttribute('aria-valuenow'), null)
      assert.equal((await unknown.locator('[data-capacity-used]').textContent()).trim(), '—')
      await unknown.locator('[data-account-mobile-health]').getByText('暂无样本', { exact: true }).waitFor()
      await page.getByRole('button', { name: '上一页', exact: true }).click()
      await first().waitFor()
      await first().getByRole('button', { name: '展开统计', exact: true }).click()
      await first().getByText('保本价', { exact: true }).waitFor()
      assert.equal(await first().locator('dd[data-column-key="health"]').count(), 0, 'health is shown once on the card, not duplicated in details')
      for (const width of [320, 390]) {
        await page.setViewportSize({ width, height: 900 })
        const detail = first().getByRole('region', { name: '账号详细统计', exact: true })
        const cost = detail.locator('dd[data-column-key="purchaseCost"]')
        const cellBox = await cost.boundingBox()
        const valueBox = await cost.locator('[data-account-breakeven]').boundingBox()
        assert.ok(Math.abs(cellBox.x + cellBox.width - valueBox.x - valueBox.width) <= 1, `breakeven text is right-aligned at ${width}px`)
        await detail.screenshot({ path: `${output}/expanded-${width}.png` })
        assert.ok(await first().evaluate(el => el.scrollWidth <= el.clientWidth + 1), `expanded card overflow ${width}`)
        const layout = await detail.evaluate(el => ({
          fits: el.scrollWidth <= el.clientWidth + 1,
          overflow: [...el.querySelectorAll('*')].filter(child => child.getBoundingClientRect().right > el.getBoundingClientRect().right + 1).slice(0, 10).map(child => ({ tag: child.tagName, class: child.className, width: child.getBoundingClientRect().width })),
        }))
        assert.ok(layout.fits, `expanded detail overflow ${width}: ${JSON.stringify(layout.overflow)}`)
      }
      const selectedBefore = await first().getByRole('checkbox').isChecked()
      const readCount = reads.length
      await first().getByRole('button', { name: '刷新额度', exact: true }).click()
      await first().getByText('最近刷新: mobile-refresh-complete', { exact: false }).waitFor()
      assert.equal(reads.length, readCount, 'quota refresh retains the row without full-list fetch')
      assert.equal(await first().getByRole('checkbox').isChecked(), selectedBefore)
      await first().getByRole('button', { name: '收起统计', exact: true }).waitFor()
      await page.setViewportSize({ width: 1440, height: 1000 })
      const desktop = page.locator(`tr[data-row-key="${rows[0].id}"]`)
      await desktop.waitFor()
      assert.equal(await page.locator('[data-account-mobile-list]').count(), 0)
      assert.equal(await desktop.getByRole('checkbox').isChecked(), true)
      await desktop.getByRole('button', { name: '收起统计', exact: true }).waitFor()
      assert.equal(reads.length, readCount, 'viewport switches do not refetch')
      assert.equal(await page.locator('thead th[data-column-key="identity"]').count(), 1)
      assert.equal(await desktop.locator('[data-account-mobile-health]').count(), 0, 'desktop usage header has no mobile health strip')
      await page.screenshot({ path: `${output}/desktop-1440.png` })
      await page.setViewportSize({ width: 390, height: 844 })
      await first().getByRole('button', { name: '收起统计', exact: true }).click()
      await first().getByRole('button', { name: '更多操作', exact: true }).click()
      await page.getByRole('button', { name: '测试连接', exact: true }).waitFor()
      await page.keyboard.press('Escape')
      await first().getByRole('button', { name: '编辑账号', exact: true }).click()
      const editDialog = page.getByRole('dialog')
      await editDialog.getByRole('textbox', { name: '自定义账号名称', exact: true }).waitFor()
      assert.equal(await editDialog.getByRole('textbox', { name: '自定义账号名称', exact: true }).inputValue(), rows[0].customName)
      await editDialog.getByRole('button', { name: '取消未保存更改', exact: true }).click()
      await editDialog.waitFor({ state: 'hidden' })
      await first().getByRole('button', { name: '删除账号', exact: true }).click()
      await page.getByRole('alertdialog').getByRole('button', { name: '取消', exact: true }).click()
      assert.equal(await first().getByRole('checkbox').isChecked(), true, 'actions never toggle selection')
      const switchControl = first().getByRole('switch')
      await switchControl.locator('..').click()
      await page.waitForFunction(() => document.querySelector('[data-account-card] input[role="switch"]')?.checked === false)
      assert.equal(writes.at(-1).enabled, false)
      const autoStopped = page.locator(`[data-account-card="${rows[2].id}"]`)
      assert.equal(await autoStopped.getByRole('switch').isDisabled(), true)
      await chooseSort('账号')
      await page.waitForFunction(() => !document.querySelector('[data-account-mobile-list][aria-busy="true"]'))
      assert.equal(reads.at(-1).sortBy, 'email')
      const sorted = page.waitForResponse(response => response.url().includes('sortDirection=desc') && response.url().includes('/api/admin/accounts?'))
      await page.getByRole('button', { name: '切换为降序', exact: true }).click()
      await sorted
      assert.equal(reads.at(-1).sortDirection, 'desc')
      const searched = page.waitForResponse(response => response.url().includes('mobile-2') && response.url().includes('/api/admin/accounts?'))
      await page.getByPlaceholder('搜索账号', { exact: true }).fill('mobile-2')
      await searched
      await page.waitForFunction(() => document.querySelectorAll('[data-account-card]').length === 1)
      assert.equal(await page.locator('[data-account-card]').count(), 1)
      empty = true
      await page.reload()
      await page.getByText('暂无账号数据', { exact: true }).waitFor()
      assert.equal(await page.locator('[data-account-card]').count(), 0)
      assert.deepEqual(errors, [])
      assert.deepEqual(unexpected, [])
      process.stdout.write('Mobile cards: themes, widths, state continuity, quota refresh, selection, actions, sorting and empty state passed.\n')
    }
    catch (error) {
      await page.screenshot({ path: `${output}/failure.png`, fullPage: true })
      console.error({ errors, unexpected, reads: reads.length })
      throw error
    }
    finally { await page.close() }
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
