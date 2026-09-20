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
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-relogin-template-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
  const errors = []
  const pushes = []
  const saves = []
  page.on('pageerror', error => errors.push(error.message))
  page.on('request', (request) => {
    if (request.url().endsWith('/relogin/push'))
      pushes.push(request.postDataJSON())
    if (request.url().endsWith('/templates/save'))
      saves.push(request.postDataJSON())
  })
  const base = process.env.QA_BASE_URL || 'http://127.0.0.1:5216'
  const modal = page.getByRole('dialog')
  const confirm = page.getByRole('alertdialog')
  const row = id => page.locator(`tbody tr[data-row-key="${id}"]`)
  try {
    await page.goto(`${base}/relogin`)
    await row('new-one').waitFor()
    await page.getByRole('button', { name: '账号模板', exact: true }).click()
    await modal.getByRole('button', { name: '新建模板', exact: true }).click()
    await modal.getByRole('textbox', { name: '模板名称', exact: true }).fill('Team 模板')
    await modal.getByRole('switch', { name: '切换 Turn State 注入', exact: true }).locator('..').click()
    await modal.getByRole('spinbutton', { name: '账号并发限制' }).fill('6')
    await modal.getByRole('spinbutton', { name: '账号调度权重' }).fill('17')
    await modal.getByRole('checkbox', { name: '测试分组', exact: true }).locator('..').click()
    await modal.getByRole('button', { name: '保存模板', exact: true }).click()
    await modal.getByText('Team 模板', { exact: true }).waitFor()
    assert.equal(saves.length, 1)
    assert.equal(saves[0].config.turnStateInjectionEnabled, true)
    assert.equal('turnStateParameters' in saves[0].config, false)
    assert.equal(saves[0].config.concurrencyLimit, 6)
    assert.equal(saves[0].config.groupIds.length, 1)
    await modal.getByRole('button', { name: '关闭', exact: true }).last().click()
    await page.reload()
    await page.getByRole('button', { name: '账号模板', exact: true }).click()
    await modal.getByText('Team 模板', { exact: true }).waitFor()
    await modal.getByRole('button', { name: '编辑模板', exact: true }).click()
    assert.equal(await modal.getByRole('switch', { name: '切换 Turn State 注入', exact: true }).isChecked(), true)
    assert.equal(await modal.getByRole('spinbutton', { name: '账号并发限制' }).inputValue(), '6')
    await modal.getByRole('spinbutton', { name: '账号并发限制' }).fill('0')
    await modal.getByRole('button', { name: '保存模板', exact: true }).click()
    await modal.getByRole('alert').waitFor()
    assert.equal(saves.length, 1, 'invalid config never submitted')
    await modal.getByRole('spinbutton', { name: '账号并发限制' }).fill('9')
    await modal.getByRole('button', { name: '保存模板', exact: true }).click()
    await modal.getByText('Team 模板', { exact: true }).waitFor()
    assert.equal(saves[1].selection.revision, 1)
    await modal.getByRole('button', { name: '关闭', exact: true }).last().click()

    await page.getByRole('checkbox', { name: '全选当前页', exact: true }).locator('..').click()
    await page.getByRole('button', { name: '批量推送', exact: true }).click()
    await confirm.waitFor()
    assert.ok((await confirm.textContent()).includes('新增 2 项，更新已有账号 1 项'))
    await confirm.getByRole('combobox', { name: '新增账号模板' }).click()
    await page.getByRole('option', { name: 'Team 模板', exact: true }).click()
    await page.getByRole('option', { name: 'Team 模板', exact: true }).waitFor({ state: 'hidden' })
    await confirm.getByText('测试分组', { exact: true }).waitFor()
    assert.equal(await confirm.locator('dt').filter({ hasText: 'State 开关' }).locator('+ dd').textContent().then(text => text.trim()), '开启')
    await confirm.getByRole('button', { name: '取消', exact: true }).click()
    await confirm.waitFor({ state: 'hidden' })
    assert.equal(pushes.length, 0)
    await page.getByRole('button', { name: '批量推送', exact: true }).click()
    assert.ok((await confirm.getByRole('combobox', { name: '新增账号模板' }).textContent()).includes('不使用模板'))
    await confirm.getByRole('combobox', { name: '新增账号模板' }).click()
    await page.getByRole('option', { name: 'Team 模板', exact: true }).click()
    for (const theme of ['light', 'dark']) {
      await page.setViewportSize({ width: 1440, height: 1000 })
      if (await page.evaluate(() => document.documentElement.dataset.theme) !== theme) {
        await page.getByRole('button', { name: theme === 'dark' ? '切换暗黑模式' : '切换浅色模式', exact: true })
          .evaluate(button => button.click())
      }
      await page.waitForFunction(theme => document.documentElement.dataset.theme === theme, theme)
      await page.getByRole('option', { name: 'Team 模板', exact: true }).waitFor({ state: 'hidden' })
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: width < 500 ? 844 : 1000 })
        assert.ok(await confirm.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
        await page.screenshot({ path: `${output}/push-${theme}-${width}.png`, fullPage: true })
      }
    }
    await confirm.getByRole('button', { name: '确认', exact: true }).click()
    await confirm.waitFor({ state: 'hidden' })
    assert.equal(pushes.length, 1)
    assert.equal(pushes[0].template.revision, 2)
    assert.equal(pushes[0].ids.length, 3)
    await page.setViewportSize({ width: 1440, height: 1000 })
    await row('new-one').getByRole('button', { name: '推送', exact: true }).click()
    await confirm.getByRole('button', { name: '确认', exact: true }).click()
    await confirm.waitFor({ state: 'hidden' })
    assert.equal(pushes[1].template, undefined)
    await row('existing').getByRole('button', { name: '推送', exact: true }).click()
    assert.equal(await confirm.getByRole('combobox', { name: '新增账号模板' }).count(), 0)
    await confirm.getByRole('button', { name: '取消', exact: true }).click()
    await confirm.waitFor({ state: 'hidden' })

    await page.route('**/dev/api/admin/relogin/push', route => route.fulfill({
      status: 409,
      json: { code: 409, message: '模板已修改，请重新选择并确认配置', data: null },
    }))
    await row('new-one').getByRole('button', { name: '推送', exact: true }).click()
    await confirm.getByRole('combobox', { name: '新增账号模板' }).click()
    await page.getByRole('option', { name: 'Team 模板', exact: true }).click()
    await page.getByRole('option', { name: 'Team 模板', exact: true }).waitFor({ state: 'hidden' })
    await confirm.getByRole('button', { name: '确认', exact: true }).click()
    await confirm.getByRole('alert').filter({ hasText: '模板已修改' }).waitFor()
    assert.equal(await confirm.isVisible(), true, 'failed push preserves the confirmation')
    await confirm.getByRole('button', { name: '取消', exact: true }).click()
    await confirm.waitFor({ state: 'hidden' })
    await page.unroute('**/dev/api/admin/relogin/push')

    await page.getByRole('button', { name: '账号模板', exact: true }).click()
    await modal.getByRole('button', { name: '编辑模板', exact: true }).click()
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: width < 500 ? 844 : 1000 })
      assert.ok(await modal.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
      await page.screenshot({ path: `${output}/edit-${width}.png`, fullPage: true })
    }
    await modal.getByRole('button', { name: '返回', exact: true }).click()
    await modal.getByRole('button', { name: '删除模板', exact: true }).click()
    await confirm.getByRole('button', { name: '确认', exact: true }).click()
    await confirm.waitFor({ state: 'hidden' })
    await modal.getByText('暂无账号模板', { exact: true }).waitFor()
    assert.deepEqual(errors, [])
    process.stdout.write(`Passed: template CRUD, reload persistence, validation, cancel, mixed/no-template/existing pushes, responsive layout. Screenshots: ${output}\n`)
  }
  catch (error) {
    await page.screenshot({ path: `${output}/failure.png`, fullPage: true })
    throw error
  }
  finally { await browser.close() }
}

main().catch((error) => {
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
