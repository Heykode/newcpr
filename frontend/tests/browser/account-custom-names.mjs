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
  const base = process.env.QA_BASE_URL || 'http://127.0.0.1:5240'
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-custom-name-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
  const errors = []
  const pushes = []
  const updates = []
  page.on('pageerror', error => errors.push(error.message))
  page.on('request', (request) => {
    if (request.url().endsWith('/relogin/push'))
      pushes.push(request.postDataJSON())
    if (request.url().endsWith('/accounts/batch-update'))
      updates.push(request.postDataJSON())
  })
  const dialog = page.getByRole('dialog')
  const confirm = page.getByRole('alertdialog')
  const row = id => page.locator(`tbody tr[data-row-key="${id}"]`)
  const name = () => dialog.getByRole('textbox', { name: '自定义账号名称', exact: true })
  const first = () => row('acct_sample_0')
  async function saveName(value) {
    await first().getByRole('button', { name: '编辑账号', exact: true }).click()
    await name().fill(value)
    await dialog.getByRole('button', { name: '保存账号设置', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
  }
  async function layouts(surface, label) {
    for (const theme of ['light', 'dark']) {
      await page.setViewportSize({ width: 1440, height: 1000 })
      if (await page.evaluate(() => document.documentElement.dataset.theme) !== theme) {
        await page.getByRole('button', { name: theme === 'dark' ? '切换暗黑模式' : '切换浅色模式', exact: true })
          .evaluate(button => button.click())
      }
      await page.waitForFunction(theme => document.documentElement.dataset.theme === theme, theme)
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: width < 500 ? 844 : 1000 })
        assert.ok(await surface.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
        const bounds = await surface.boundingBox()
        assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= width + 1)
        await page.screenshot({ path: `${output}/${label}-${theme}-${width}.png`, fullPage: true })
      }
    }
    await page.setViewportSize({ width: 1440, height: 1000 })
  }
  try {
    await page.goto(`${base}/accounts`)
    await first().waitFor()
    await saveName('  本地测试名称  ')
    await first().getByText('本地测试名称', { exact: true }).waitFor()
    assert.equal(await first().getByText('sample-0@example.invalid', { exact: true }).count(), 1)
    await page.reload()
    await first().getByText('本地测试名称', { exact: true }).waitFor()
    await first().getByRole('button', { name: '编辑账号', exact: true }).click()
    assert.equal(await name().inputValue(), '本地测试名称')
    await layouts(dialog, 'edit')
    await dialog.getByRole('button', { name: '取消未保存更改', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    await saveName('')
    await first().getByText('sample-0', { exact: true }).waitFor()

    for (const id of ['acct_sample_0', 'acct_sample_1'])
      await row(id).getByRole('checkbox', { name: '选择账号', exact: true }).locator('..').click()
    await page.getByRole('button', { name: '批量编辑账号', exact: true }).click()
    assert.equal(await name().isDisabled(), true)
    await dialog.getByRole('checkbox', { name: '应用账号名称更改', exact: true }).locator('..').click()
    await name().fill('批量测试名称')
    await layouts(dialog, 'batch')
    await dialog.getByRole('button', { name: '保存更改', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    await first().getByText('批量测试名称', { exact: true }).waitFor()
    await row('acct_sample_1').getByText('批量测试名称', { exact: true }).waitFor()
    assert.deepEqual(updates.at(-1), { accountIds: ['acct_sample_0', 'acct_sample_1'], customName: '批量测试名称' })

    await page.getByRole('button', { name: '账号模板', exact: true }).click()
    await page.getByRole('button', { name: '管理模板', exact: true }).click()
    await dialog.getByRole('button', { name: '新建模板', exact: true }).click()
    assert.equal(await name().count(), 0)
    await dialog.getByRole('button', { name: '关闭', exact: true }).last().click()
    await dialog.waitFor({ state: 'hidden' })

    await page.getByRole('button', { name: '导入账号', exact: true }).click()
    await name().waitFor()
    await dialog.getByRole('button', { name: 'OpenAI', exact: true }).click()
    await name().fill('x'.repeat(129))
    assert.equal(await dialog.getByRole('button', { name: '继续导入', exact: true }).isDisabled(), true)
    await dialog.getByRole('alert').filter({ hasText: '128' }).waitFor()
    await name().fill('导入批次名称')
    await layouts(dialog, 'import')
    await dialog.getByRole('button', { name: '取消', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })

    await page.goto(`${base}/relogin`)
    await row('new-one').waitFor()
    await row('new-one').getByRole('button', { name: '推送', exact: true }).click()
    const batchName = () => confirm.getByRole('textbox', { name: '本批账号名称', exact: true })
    await batchName().fill('临时批次')
    await confirm.getByRole('button', { name: '取消', exact: true }).click()
    await confirm.waitFor({ state: 'hidden' })
    assert.equal(pushes.length, 0)
    await row('new-one').getByRole('button', { name: '推送', exact: true }).click()
    assert.equal(await batchName().inputValue(), '')
    await confirm.getByRole('button', { name: '确认', exact: true }).click()
    await confirm.waitFor({ state: 'hidden' })
    assert.equal('customName' in pushes[0], false)

    await page.getByRole('checkbox', { name: '全选当前页', exact: true }).locator('..').click()
    await page.getByRole('button', { name: '批量推送', exact: true }).click()
    await batchName().fill('x'.repeat(129))
    await confirm.getByRole('button', { name: '确认', exact: true }).click()
    await confirm.getByRole('alert').filter({ hasText: '128' }).waitFor()
    assert.equal(pushes.length, 1)
    await batchName().fill('  新入池批次  ')
    await layouts(confirm, 'push')
    await confirm.getByRole('button', { name: '确认', exact: true }).click()
    await confirm.waitFor({ state: 'hidden' })
    assert.equal(pushes[1].customName, '新入池批次')
    assert.equal(pushes[1].ids.length, 3)
    assert.equal('template' in pushes[1], false)
    await row('existing').getByRole('button', { name: '推送', exact: true }).click()
    assert.equal(await batchName().count(), 0)
    await confirm.getByRole('button', { name: '确认', exact: true }).click()
    await confirm.waitFor({ state: 'hidden' })
    assert.equal('customName' in pushes[2], false)

    const longName = 'LongAccountName'.repeat(8)
    await page.goto(`${base}/accounts`)
    await first().waitFor()
    await saveName(longName)
    await first().getByText(longName, { exact: true }).waitFor()
    const title = first().getByTitle(longName, { exact: true })
    assert.ok(await title.evaluate(element => element.scrollWidth > element.clientWidth))
    await page.screenshot({ path: `${output}/long-name.png`, fullPage: true })
    await saveName('本批测试账号')
    assert.deepEqual(errors, [])
    process.stdout.write(`${JSON.stringify({ result: 'passed', pushes: pushes.length, output })}\n`)
  }
  catch (error) {
    await page.screenshot({ path: `${output}/failure.png`, fullPage: true })
    throw error
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
