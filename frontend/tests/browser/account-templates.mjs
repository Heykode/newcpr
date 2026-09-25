import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  })
  const base = process.env.QA_BASE_URL || 'http://127.0.0.1:5238'
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-shared-template-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
  const errors = []
  const applications = []
  const saves = []
  const name = '共享账号模板'
  page.on('pageerror', error => errors.push(error.message))
  page.on('request', (request) => {
    if (request.url().endsWith('/accounts/apply-template'))
      applications.push(request.postDataJSON())
    if (request.url().endsWith('/templates/save'))
      saves.push(request.postDataJSON())
  })
  const dialog = page.getByRole('dialog')
  const menu = page.getByLabel('账号模板菜单', { exact: true })
  const row = index => page.locator(`tbody tr[data-row-key="acct_sample_${index}"]`)
  const toggle = index => row(index).getByRole('checkbox', { name: '选择账号', exact: true }).locator('..').click()
  const openMenu = () => page.getByRole('button', { name: '账号模板', exact: true }).click()
  const apply = () => menu.getByRole('button', { name: `应用模板：${name}`, exact: true })
  async function manage() {
    await openMenu()
    await menu.getByRole('button', { name: '管理模板', exact: true }).click()
    await dialog.waitFor()
  }
  async function readAccounts() {
    const response = await page.request.get(`${base}/dev/api/admin/accounts`)
    return (await response.json()).data.items
  }
  try {
    const catalog = await (await page.request.get(`${base}/dev/api/admin/relogin/templates`)).json()
    for (const template of catalog.data.filter(row => row.config.name === name)) {
      await page.request.post(`${base}/dev/api/admin/relogin/templates/delete`, {
        data: { id: template.id, revision: template.revision },
      })
    }
    await page.goto(`${base}/accounts`)
    await row(0).waitFor()
    const original = await readAccounts()
    await manage()
    await dialog.getByRole('button', { name: '新建模板', exact: true }).click()
    await dialog.getByRole('textbox', { name: '模板名称', exact: true }).fill(name)
    assert.equal(await dialog.getByRole('switch', { name: '切换 Turn State 注入', exact: true }).count(), 0)
    await dialog.getByRole('spinbutton', { name: '账号并发限制' }).fill('6')
    await dialog.getByRole('spinbutton', { name: '账号调度权重' }).fill('17')
    await dialog.getByRole('checkbox', { name: '测试分组', exact: true }).locator('..').click()
    await dialog.getByRole('button', { name: '保存模板', exact: true }).click()
    await dialog.getByText(name, { exact: true }).waitFor()
    assert.equal('turnStateInjectionEnabled' in saves[0].config, false)
    assert.equal('turnStateParameters' in saves[0].config, false)
    await dialog.getByRole('button', { name: '关闭', exact: true }).last().click()
    await openMenu()
    await apply().waitFor()
    assert.equal(await apply().isDisabled(), true)
    await page.keyboard.press('Escape')

    await toggle(0)
    await openMenu()
    await apply().waitFor()
    await apply().evaluate((button) => {
      button.click()
      button.click()
    })
    await menu.waitFor({ state: 'hidden' })
    assert.equal(applications.length, 1, 'duplicate click must not duplicate application')
    assert.deepEqual(applications[0].accountIds, ['acct_sample_0'])
    assert.deepEqual(Object.keys(applications[0]).sort(), ['accountIds', 'template'])
    let current = await readAccounts()
    assert.equal(current[0].responsesUpstream, original[0].responsesUpstream)
    assert.equal(current[0].concurrencyLimit, 6)
    assert.equal(current[0].weight, 17)
    assert.equal(current[0].groups.length, 1)

    await toggle(1)
    await openMenu()
    await apply().click()
    await menu.waitFor({ state: 'hidden' })
    assert.deepEqual(applications[1].accountIds.sort(), ['acct_sample_0', 'acct_sample_1'])
    await manage()
    await dialog.getByRole('button', { name: '编辑模板', exact: true }).click()
    assert.equal(await dialog.getByRole('switch', { name: '切换 Turn State 注入', exact: true }).count(), 0)
    await dialog.getByRole('spinbutton', { name: '账号并发限制' }).fill('')
    await dialog.getByRole('checkbox', { name: '测试分组', exact: true }).locator('..').click()
    await dialog.getByRole('button', { name: '保存模板', exact: true }).click()
    await dialog.getByText(name, { exact: true }).waitFor()
    await dialog.getByRole('button', { name: '关闭', exact: true }).last().click()
    await openMenu()
    await apply().click()
    await menu.waitFor({ state: 'hidden' })
    current = await readAccounts()
    for (const index of [0, 1]) {
      assert.equal(current[index].responsesUpstream, original[index].responsesUpstream)
      assert.equal(current[index].concurrencyLimit, null)
      assert.equal(current[index].groups.length, 0)
      for (const field of ['customName', 'accountId', 'userId', 'reloginCount', 'hasRefreshToken'])
        assert.deepEqual(current[index][field], original[index][field])
    }
    assert.equal(applications[2].template.revision, 2)

    const applyPattern = '**/api/admin/accounts/apply-template'
    await page.route(applyPattern, route => route.fulfill({
      status: 409,
      contentType: 'application/json',
      body: JSON.stringify({ code: 409, message: '模板已修改，请重新选择', data: null }),
    }))
    await openMenu()
    await apply().click()
    await menu.getByRole('alert').getByText('模板已修改，请重新选择', { exact: true }).waitFor()
    assert.equal(await menu.isVisible(), true)
    await page.unroute(applyPattern)
    await page.keyboard.press('Escape')

    // Keep selection across pages; the menu must send IDs absent from the current page.
    const accountsPattern = '**/api/admin/accounts?*'
    await page.route(accountsPattern, async (route) => {
      const response = await route.fetch()
      const body = await response.json()
      const requestedPage = Number(new URL(route.request().url()).searchParams.get('page') || 1)
      body.data.items = [body.data.items[requestedPage - 1]]
      body.data.page = { page: requestedPage, pageSize: 20, total: 21, totalPages: 2 }
      await route.fulfill({ response, json: body })
    })
    await page.reload()
    await row(0).waitFor()
    await toggle(0)
    await page.getByRole('button', { name: '下一页', exact: true }).click()
    await row(1).waitFor()
    await toggle(1)
    await openMenu()
    await apply().click()
    await menu.waitFor({ state: 'hidden' })
    assert.deepEqual(applications.at(-1).accountIds.sort(), ['acct_sample_0', 'acct_sample_1'])
    await page.unroute(accountsPattern)

    for (const theme of ['light', 'dark']) {
      await page.setViewportSize({ width: 1440, height: 1000 })
      if (await page.evaluate(() => document.documentElement.dataset.theme) !== theme)
        await page.getByRole('button', { name: theme === 'dark' ? '切换暗黑模式' : '切换浅色模式', exact: true }).evaluate(button => button.click())
      await page.waitForFunction(theme => document.documentElement.dataset.theme === theme, theme)
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: width < 500 ? 844 : 1000 })
        await openMenu()
        await apply().waitFor()
        assert.ok(await menu.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
        const bounds = await menu.boundingBox()
        assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= width)
        await page.screenshot({ path: `${output}/menu-${theme}-${width}.png`, fullPage: true })
        await page.keyboard.press('Escape')
      }
    }

    await page.setViewportSize({ width: 1440, height: 1000 })
    await page.goto(`${base}/relogin`)
    await page.getByRole('button', { name: '账号模板', exact: true }).click()
    await dialog.getByText(name, { exact: true }).waitFor()
    await dialog.getByRole('button', { name: '编辑模板', exact: true }).click()
    assert.equal(await dialog.getByRole('switch', { name: '切换 Turn State 注入', exact: true }).count(), 0)
    await page.screenshot({ path: `${output}/shared-editor.png`, fullPage: true })
    assert.deepEqual(errors, [])
    process.stdout.write(`${JSON.stringify({ result: 'passed', applications: applications.length, output })}\n`)
  }
  finally { await browser.close() }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
