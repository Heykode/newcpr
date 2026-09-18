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
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-account-relogin-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
  const errors = []
  const failedAssets = []
  const writes = []
  page.on('pageerror', error => errors.push(error.message))
  page.on('response', (response) => {
    if (response.status() >= 400 && ['font', 'image', 'stylesheet', 'script'].includes(response.request().resourceType()))
      failedAssets.push(new URL(response.url()).pathname)
  })
  page.on('request', (request) => {
    if (request.url().endsWith('/relogin/accounts/queue'))
      writes.push(request.postDataJSON())
  })
  const base = process.env.QA_BASE_URL || 'http://127.0.0.1:5208'
  const menu = index => page.locator('td[data-column-key="actions"]').nth(index).getByRole('button', { name: '更多操作' })
  const actionsMenu = page.getByRole('group', { name: '账号操作' })
  try {
    await page.goto(`${base}/accounts`)
    await page.locator('td[data-column-key="actions"]').first().waitFor()
    const initialCount = Number((await page.locator('td[data-column-key="reloginCount"]').first().textContent()).trim())
    await menu(2).click()
    assert.equal(await actionsMenu.getByRole('button', { name: '失效重登', exact: true }).count(), 0)
    await page.keyboard.press('Escape')
    await menu(0).click()
    await actionsMenu.getByRole('button', { name: '失效重登', exact: true }).click()
    const modal = page.getByRole('alertdialog')
    await modal.waitFor()
    assert.ok((await modal.textContent()).includes('sample-0@example.invalid'))
    assert.ok((await modal.textContent()).includes('workspace-0'))
    await modal.getByRole('button', { name: '取消', exact: true }).click()
    assert.equal(writes.length, 0)
    await menu(0).click()
    await actionsMenu.getByRole('button', { name: '失效重登', exact: true }).click()
    await modal.getByRole('button', { name: '重登并同步', exact: true }).click()
    await modal.waitFor({ state: 'hidden' })
    assert.equal(writes.length, 1)
    assert.equal(writes[0].target.account_id, 'acct_sample_0')
    assert.equal(writes[0].target.workspace_id, 'workspace-0')
    await menu(0).click()
    const busy = actionsMenu.getByRole('button', { name: '重登处理中', exact: true })
    await busy.waitFor()
    assert.equal(await busy.isDisabled(), true)
    await page.keyboard.press('Escape')
    await page.waitForFunction(expected => document.querySelector('td[data-column-key="reloginCount"]')?.textContent?.trim() === String(expected), initialCount + 1)
    assert.equal(writes.length, 1)

    for (const theme of ['light', 'dark']) {
      await page.setViewportSize({ width: 1440, height: 1000 })
      if (await page.evaluate(() => document.documentElement.dataset.theme) !== theme) {
        await page.getByRole('button', { name: theme === 'dark' ? '切换暗黑模式' : '切换浅色模式', exact: true }).click()
      }
      await page.waitForFunction(theme => document.documentElement.dataset.theme === theme, theme)
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: width < 500 ? 844 : 1000 })
        await menu(0).click()
        await page.screenshot({ path: `${output}/menu-${theme}-${width}.png`, fullPage: true })
        await actionsMenu.getByRole('button', { name: '失效重登', exact: true }).click()
        await modal.waitFor()
        const fits = await modal.evaluate(element => element.scrollWidth <= element.clientWidth + 1)
        assert.ok(fits, `${width}: modal content fits`)
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1))
        await page.screenshot({ path: `${output}/confirm-${theme}-${width}.png`, fullPage: true })
        await modal.getByRole('button', { name: '取消', exact: true }).click()
      }
    }
    assert.deepEqual(errors, [])
    assert.deepEqual(failedAssets, [])
    assert.equal(writes.length, 1)
    process.stdout.write(`Passed: hidden/visible menu, confirmation/cancel, exact target, busy guard, success count, desktop/mobile light/dark. Screenshots: ${output}\n`)
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
