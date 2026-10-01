import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir, readFile } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { reloginEntries } from '../fixtures/relogin-count-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const port = process.env.QA_PORT || '5192'
  const base = `http://127.0.0.1:${port}`
  const server = spawn(process.execPath, ['tests/relogin-count-preview.mjs'], {
    env: { ...process.env, QA_PORT: port },
    stdio: 'ignore',
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-2fa-ui'
  await mkdir(output, { recursive: true })
  let browser
  try {
    for (let i = 0; i < 100; i++) {
      try {
        if ((await fetch(`${base}/accounts`)).ok)
          break
      }
      catch {}
      await new Promise(resolve => setTimeout(resolve, 100))
    }
    browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH || undefined })
    const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, reducedMotion: 'reduce' })
    const errors = []
    const submissions = []
    const exports = []
    let stage = 'queued'
    let reloginPage = false
    let accountReads = 0
    page.on('pageerror', error => errors.push(error.message))
    const fulfill = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
    await page.route('**/dev/api/admin/proxies?*', route => fulfill(route, {
      items: [],
      page: { page: 1, pageSize: 200, total: 0, totalPages: 0 },
    }))
    await page.route('**/dev/api/admin/accounts?*', (route) => {
      accountReads++
      return route.continue()
    })
    await page.route('**/dev/api/admin/relogin', route => fulfill(route, {
      settings: { concurrency: 1, paused: false, maxRetries: 2, retryIntervalMinutes: 5 },
      items: reloginPage
        ? reloginEntries
        : [{
            ...reloginEntries[0],
            id: 'enrollment-fixture',
            email: 'import@example.invalid',
            status: stage === 'synced' ? 'ready' : stage,
            enrollmentPending: stage !== 'synced',
            poolStatus: stage === 'synced' ? 'synced' : 'not_in_pool',
            message: stage === 'synced' ? '凭据已同步到号池' : '等待登录验证并入池',
          }],
    }))
    await page.route('**/dev/api/admin/relogin/enroll', (route) => {
      submissions.push(route.request().postDataJSON())
      return fulfill(route, { ids: ['enrollment-fixture'] })
    })
    await page.route('**/dev/api/admin/relogin/export', (route) => {
      const body = route.request().postDataJSON()
      exports.push(body)
      assert.equal(body.confirm, 'export_sensitive_relogin')
      return fulfill(route, {
        files: [{
          name: body.format === 'json' ? 'relogin-accounts.json' : 'relogin-2fa.txt',
          content: body.format === 'json' ? '{"documents":[]}' : 'selected@example.invalid----synthetic----JBSWY3DPEHPK3PXP\n',
        }],
      })
    })
    await page.goto(`${base}/accounts`)
    await page.getByRole('button', { name: '导入账号', exact: true }).click()
    const dialog = page.getByRole('dialog')
    await dialog.getByRole('button', { name: 'OpenAI', exact: true }).click()
    await dialog.getByRole('checkbox', { name: '指定 Excel 设置', exact: true }).locator('..').click()
    assert.equal(await dialog.getByRole('switch', { name: '缓存写入按普通输入计费', exact: true }).isChecked(), true)
    await dialog.getByRole('switch', { name: '切换 Excel 入口', exact: true }).locator('..').click()
    await dialog.getByRole('combobox', { name: 'Excel遇到HTTP 403', exact: true }).click()
    await page.getByRole('option', { name: '关闭Excel模式', exact: true }).click()
    await dialog.getByRole('button', { name: '继续导入', exact: true }).click()
    await dialog.getByRole('radio', { name: '2FA', exact: true }).click()
    const material = [
      `import@example.invalid----synthetic-${'x'.repeat(180)}----JBSWY3DPEHPK3PXP`,
      '',
      '\uFEFFsecond@example.invalid---- synthetic password ----jbsw\u00A0y3dp\u3000ehpk3pxp',
    ].join('\n')
    const input = dialog.getByRole('textbox', { name: '2FA 账号', exact: true })
    await input.fill(material)
    assert.equal(await input.getAttribute('wrap'), 'off')
    assert.equal(await input.getAttribute('spellcheck'), 'false')
    await dialog.getByText('确认更新已有账号的 2FA 资料', { exact: true }).waitFor()
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 900 })
      assert.ok(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
      assert.equal(await input.inputValue(), material)
      assert.ok(await input.evaluate(element =>
        getComputedStyle(element).whiteSpace === 'pre'
        && element.scrollWidth > element.clientWidth,
      ))
      assert.ok(await dialog.getByRole('radiogroup', { name: '账号添加方式' })
        .locator('button > span')
        .evaluateAll(labels => labels.every(label => label.scrollWidth <= label.clientWidth)))
      await page.screenshot({ path: `${output}/import-${width}.png` })
    }
    await dialog.getByRole('button', { name: '登录并导入', exact: true }).evaluate((button) => {
      button.click()
      button.click()
    })
    await dialog.waitFor({ state: 'hidden' })
    assert.equal(submissions.length, 1)
    assert.equal(submissions[0].text, material)
    assert.equal(submissions[0].settings.excel403Action, 'disable_excel')
    assert.equal(submissions[0].settings.excelCacheCreationAsInput, true)
    const progress = page.getByRole('region', { name: '2FA导入进度' })
    await progress.getByText('等待登录验证并入池', { exact: true }).waitFor()
    const beforeCompletion = accountReads
    stage = 'synced'
    await progress.getByText(/1\/1 已入池/).waitFor({ timeout: 10000 })
    await page.waitForTimeout(500)
    assert.ok(accountReads > beforeCompletion, 'settled enrollment refreshes the pool')
    await page.getByRole('button', { name: '导入账号', exact: true }).click()
    await dialog.getByRole('button', { name: 'OpenAI', exact: true }).click()
    await dialog.getByRole('button', { name: '继续导入', exact: true }).click()
    await dialog.getByRole('radio', { name: '2FA', exact: true }).click()
    assert.equal(await dialog.getByRole('textbox', { name: '2FA 账号', exact: true }).inputValue(), '')
    await dialog.getByRole('button', { name: '关闭', exact: true }).click()
    reloginPage = true
    await page.setViewportSize({ width: 1440, height: 900 })
    await page.goto(`${base}/relogin`)
    assert.equal(await page.getByRole('button', { name: '导出JSON', exact: true }).isDisabled(), true)
    for (const entry of reloginEntries.slice(0, 2))
      await page.locator(`tr[data-row-key="${entry.id}"]`).getByRole('checkbox').first().locator('..').click()
    const confirm = page.getByRole('alertdialog')
    await page.getByRole('button', { name: '导出JSON', exact: true }).click()
    await confirm.getByRole('button', { name: '取消', exact: true }).click()
    assert.equal(exports.length, 0, 'cancel does not read secrets')
    for (const format of ['json', 'two_fa']) {
      await page.getByRole('button', { name: format === 'json' ? '导出JSON' : '导出2FA', exact: true }).click()
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 900 })
        assert.ok(await confirm.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
        await page.screenshot({ path: `${output}/export-${format}-${width}.png` })
      }
      const downloadReady = page.waitForEvent('download')
      await confirm.getByRole('button', { name: '确认', exact: true }).click()
      const download = await downloadReady
      const expectedName = format === 'json' ? 'relogin-accounts.json' : 'relogin-2fa.txt'
      assert.equal(download.suggestedFilename(), expectedName)
      await download.saveAs(`${output}/${expectedName}`)
      assert.ok((await readFile(`${output}/${expectedName}`, 'utf8')).length > 0)
      assert.deepEqual(exports.at(-1).ids, reloginEntries.slice(0, 2).map(entry => entry.id))
      assert.equal(exports.at(-1).format, format)
      await page.setViewportSize({ width: 1440, height: 900 })
    }
    assert.equal(exports.length, 2)
    assert.deepEqual(errors, [])
    process.stdout.write('Passed 2FA enrollment, defaults, double-click guard, settlement refresh, secret clearing, selected exports and mobile layouts.\n')
  }
  catch (error) {
    const page = browser?.contexts()[0]?.pages()[0]
    if (page) {
      await page.screenshot({ path: `${output}/failure.png` })
      console.error((await page.locator('body').textContent())?.slice(0, 5000))
    }
    throw error
  }
  finally {
    await browser?.close()
    server.kill('SIGTERM')
    if (server.exitCode === null)
      await new Promise(resolve => server.once('exit', resolve))
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
