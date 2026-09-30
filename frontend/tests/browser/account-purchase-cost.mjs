import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { accounts as samples } from '../fixtures/relogin-count-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const port = process.env.QA_PORT || '5249'
  const server = spawn(process.execPath, ['tests/account-relogin-preview.mjs'], { env: { ...process.env, QA_PORT: port }, stdio: 'ignore' })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-purchase-ui'
  await mkdir(output, { recursive: true })
  let browser
  try {
    let ready = false
    for (let i = 0; i < 100; i++) {
      assert.equal(server.exitCode, null, 'isolated fixture server must start successfully')
      try {
        ready = (await fetch(`http://127.0.0.1:${port}/accounts`)).ok
      }
      catch {}
      if (ready)
        break
      await new Promise(resolve => setTimeout(resolve, 100))
    }
    assert.ok(ready)
    browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH || undefined })
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
    const errors = []
    const patches = []
    const accounts = structuredClone(samples).map((account, index) => ({
      ...account,
      purchaseCost: {
        amountCny: index === 2 ? null : '50.0000000000',
        cycleAnchor: '2026-01-31',
        periodStart: '2026-09-30',
        periodEnd: '2026-10-31',
        usageUsd: index === 1 ? '0' : '200',
        breakevenCnyPerUsd: index === 0 ? '0.25' : null,
        historyComplete: true,
        historyCompleteFrom: null,
      },
    }))
    page.on('pageerror', error => errors.push(error.message))
    const fulfill = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
    await page.route('**/dev/api/admin/accounts?*', route => fulfill(route, {
      items: accounts,
      page: { page: 1, pageSize: 20, total: accounts.length, totalPages: 1 },
      summary: { total: 3, normal: 3, error: 0, disabled: 0, rateLimited: 0, quotaExhausted: 0 },
    }))
    for (const endpoint of ['update', 'batch-update']) {
      await page.route(`**/dev/api/admin/accounts/${endpoint}`, (route) => {
        const body = route.request().postDataJSON()
        patches.push(body)
        for (const account of accounts.filter(account => (body.accountIds ?? [body.accountId]).includes(account.id))) {
          if (body.purchaseCost) {
            const cost = account.purchaseCost
            cost.amountCny = body.purchaseCost.amountCny
            cost.cycleAnchor = body.purchaseCost.cycleStart ?? cost.cycleAnchor
            cost.breakevenCnyPerUsd = cost.amountCny === null || Number(cost.usageUsd) === 0 ? null : String(Number(cost.amountCny) / Number(cost.usageUsd))
          }
        }
        return fulfill(route, { accountIds: body.accountIds ?? [body.accountId], configRevision: 2 })
      })
    }
    const row = index => page.locator(`tr[data-row-key="${accounts[index].id}"]`)
    const cell = index => row(index).locator('[data-account-breakeven]')
    const dialog = page.getByRole('dialog')
    const amount = () => dialog.getByRole('textbox', { name: '每账号月成本', exact: true })
    const date = () => dialog.locator('input[aria-label="续费起算日"]')
    async function layouts(label) {
      for (const theme of ['light', 'dark']) {
        await page.setViewportSize({ width: 1440, height: 1000 })
        if (await page.locator('html').getAttribute('data-theme') !== theme)
          await page.getByRole('button', { name: theme === 'dark' ? '切换暗黑模式' : '切换浅色模式', exact: true }).evaluate(button => button.click())
        for (const width of [1440, 390, 320]) {
          await page.setViewportSize({ width, height: width < 500 ? 844 : 1000 })
          await amount().scrollIntoViewIfNeeded()
          assert.ok(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
          const bounds = await amount().boundingBox()
          assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= width + 1)
          await page.screenshot({ path: `${output}/${label}-${theme}-${width}.png`, fullPage: true })
        }
      }
      await page.setViewportSize({ width: 1440, height: 1000 })
    }
    await page.goto(`http://127.0.0.1:${port}/accounts`)
    await cell(0).waitFor()
    assert.equal(await cell(0).textContent(), '0.25')
    assert.equal(await cell(1).textContent(), '—')
    assert.equal(await cell(2).textContent(), '—')
    for (const [index, label] of ['0.25', '—', '—'].entries())
      assert.equal((await cell(index).locator('..').textContent()).trim(), label)
    const labels = await page.locator('thead th').allTextContents()
    assert.equal(labels.findIndex(text => text.trim() === '保本价'), labels.findIndex(text => text.trim() === '优先级') + 1)
    await row(0).getByRole('button', { name: '编辑账号', exact: true }).click()
    assert.equal(await amount().inputValue(), '50')
    assert.equal(await date().inputValue(), '2026-01-31')
    await layouts('edit')
    await amount().fill('60')
    await dialog.getByRole('button', { name: '保存账号设置', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    await cell(0).filter({ hasText: '0.3' }).waitFor()
    assert.deepEqual(patches[0].purchaseCost, { amountCny: '60', cycleStart: '2026-01-31' })
    await row(0).getByRole('button', { name: '编辑账号', exact: true }).click()
    await dialog.getByRole('button', { name: '保存账号设置', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    assert.equal('purchaseCost' in patches[1], false)
    for (const index of [0, 1])
      await row(index).getByRole('checkbox', { name: '选择账号', exact: true }).locator('..').click()
    await page.getByRole('button', { name: '批量编辑账号', exact: true }).click()
    assert.equal(await amount().isDisabled(), true)
    await dialog.getByRole('checkbox', { name: '修改所选账号成本', exact: true }).locator('..').click()
    await amount().fill('80')
    await layouts('batch')
    await dialog.getByRole('button', { name: '保存更改', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    assert.deepEqual(patches[2], { accountIds: accounts.slice(0, 2).map(account => account.id), purchaseCost: { amountCny: '80' } })
    await cell(0).filter({ hasText: '0.4' }).waitFor()
    await row(0).getByRole('button', { name: '编辑账号', exact: true }).click()
    await amount().fill('')
    await dialog.getByRole('button', { name: '保存账号设置', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    await cell(0).filter({ hasText: '—' }).waitFor()
    await page.getByRole('button', { name: '导入账号', exact: true }).click()
    await dialog.getByRole('button', { name: 'OpenAI', exact: true }).click()
    await amount().fill('75.5')
    await date().fill('2026-09-01')
    await layouts('import')
    await dialog.getByRole('button', { name: '取消', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    accounts[0].purchaseCost = { ...accounts[0].purchaseCost, amountCny: '50', usageUsd: '100', breakevenCnyPerUsd: '0.5', historyComplete: false }
    await page.reload()
    await cell(0).filter({ hasText: '0.5' }).waitFor()
    const costDisplay = cell(0).locator('..')
    assert.equal((await costDisplay.textContent()).trim(), '0.5')
    assert.equal(await costDisplay.getByText('元／美元', { exact: true }).count(), 0)
    assert.equal(await costDisplay.getByText('历史不完整', { exact: true }).count(), 0)
    const costDetails = await costDisplay.getAttribute('title')
    assert.match(costDetails, /每账号月成本：50 元/)
    assert.match(costDetails, /本期累计消费：100 美元/)
    assert.match(costDetails, /2026-09-30 至 2026-10-31/)
    assert.match(costDetails, /历史记录不完整，仅统计可核实消费/)
    await cell(0).scrollIntoViewIfNeeded()
    const costHeader = page.locator('thead th[data-column-key="purchaseCost"]')
    async function textBounds(locator, text) {
      return locator.evaluate((element, expected) => {
        const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT)
        while (walker.nextNode()) {
          const node = walker.currentNode
          if (node.textContent.trim() !== expected)
            continue
          const range = document.createRange()
          const start = node.textContent.indexOf(expected)
          range.setStart(node, start)
          range.setEnd(node, start + expected.length)
          const rect = range.getBoundingClientRect()
          return { x: rect.x, width: rect.width, center: rect.x + rect.width / 2 }
        }
        throw new Error(`Rendered text not found: ${expected}`)
      }, text)
    }
    const alignment = []
    for (const width of [1440, 2560, 390]) {
      await page.setViewportSize({ width, height: 1000 })
      await cell(0).scrollIntoViewIfNeeded()
      const headerText = await textBounds(costHeader, '保本价')
      for (const index of [0, 1, 2]) {
        const valueText = await textBounds(cell(index), index === 0 ? '0.5' : '—')
        const offset = valueText.center - headerText.center
        alignment.push({ width, row: index, offset })
        assert.ok(Math.abs(offset) <= 1, `breakeven text must align with its label, not just its cell: ${JSON.stringify(alignment.at(-1))}`)
      }
    }
    await page.setViewportSize({ width: 1440, height: 1000 })
    await cell(0).scrollIntoViewIfNeeded()
    await page.screenshot({ path: `${output}/table.png`, fullPage: true })
    assert.deepEqual(errors, [])
    process.stdout.write(`${JSON.stringify({ result: 'passed', patches: patches.length, alignment, output })}\n`)
  }
  finally {
    await browser?.close()
    if (server.exitCode === null) {
      const exited = once(server, 'exit')
      server.kill('SIGTERM')
      await exited
    }
  }
}
main().catch((error) => {
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
