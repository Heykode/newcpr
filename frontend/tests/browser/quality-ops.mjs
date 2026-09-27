import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const port = process.env.QA_PORT || '5197'
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-quality-ui'
  await mkdir(output, { recursive: true })
  const server = spawn(process.execPath, ['tests/relogin-count-preview.mjs'], {
    env: { ...process.env, QA_PORT: port },
    stdio: 'ignore',
  })
  let browser
  try {
    let ready = false
    for (let i = 0; i < 100; i++) {
      try {
        if ((await fetch(`http://127.0.0.1:${port}/quality-ops`)).ok) {
          ready = true
          break
        }
      }
      catch {}
      await new Promise(resolve => setTimeout(resolve, 100))
    }
    assert.ok(ready)
    browser = await chromium.launch({ headless: true })
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
    const errors = []
    page.on('pageerror', error => errors.push(error.message))
    const fulfill = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
    const now = new Date().toISOString()
    const config = {
      accountId: 'quality-fixture-account',
      model: 'fixture-long-model-name-for-layout',
      enabled: true,
      cron: '0 */6 * * *',
      timezone: 'Asia/Shanghai',
      repetitions: 2,
      prompt: 'fixture question',
      referenceAnswer: 'fixture reference',
      reasoningEffort: null,
      judgeGroupId: 'quality-group',
      judgeModel: 'fixture-judge',
      judgePrompt: 'Compare only.',
    }
    let rules = [{ id: 'quality-rule', revision: 1, config, nextRunAt: now, running: false, pending: false, lastStatus: 'correct', lastRunAt: now }]
    const run = { id: 'quality-run', ruleId: 'quality-rule', accountId: config.accountId, model: config.model, status: 'correct', startedAt: now, finishedAt: now, correct: 2, incorrect: 0, unknown: 0, requestErrors: 0 }
    let enqueues = 0
    let saves = 0
    let failHistory = false
    await page.route('**/dev/api/admin/accounts?**', route => fulfill(route, {
      items: [{ id: config.accountId, name: 'fixture-quality-account@example.test' }],
      page: { page: 1, pageSize: 50, total: 1, totalPages: 1 },
    }))
    await page.route('**/dev/api/admin/account-groups?**', route => fulfill(route, {
      items: [{ id: 'quality-group', name: '独立判题分组', enabled: true }],
      page: { page: 1, pageSize: 50, total: 1, totalPages: 1 },
    }))
    await page.route('**/dev/api/admin/quality-ops/**', async (route) => {
      const url = new URL(route.request().url())
      if (url.pathname.endsWith('/rules'))
        return fulfill(route, rules)
      if (url.pathname.endsWith('/runs') && failHistory)
        return route.fulfill({ status: 503, json: { code: 503, message: 'fixture history unavailable', data: null } })
      if (url.pathname.endsWith('/runs'))
        return fulfill(route, [run])
      if (url.pathname.endsWith('/detail')) {
        return fulfill(route, { ...run, answers: [{
          index: 1,
          answer: '<script>window.__qualityExecuted = true</script>',
          verdict: 'correct',
          reason: 'fixture comparison',
          elapsedMs: 1350,
          returnedModel: 'fixture-model',
          judgeAccountId: 'judge-account',
        }] })
      }
      if (url.pathname.endsWith('/run')) {
        enqueues++
        assert.deepEqual(route.request().postDataJSON(), { id: 'quality-rule', revision: 1 })
        rules[0].pending = true
        return fulfill(route, null)
      }
      if (url.pathname.endsWith('/save')) {
        saves++
        const body = route.request().postDataJSON()
        if (body.id === null) {
          assert.equal(body.revision, null)
          rules = [{ id: 'new-rule', revision: 1, config: body.config, nextRunAt: now, running: false, pending: false, lastStatus: null, lastRunAt: null }]
          return fulfill(route, rules[0])
        }
        assert.equal(body.id, 'quality-rule')
        assert.equal(body.config.accountId, config.accountId)
        rules[0] = { ...rules[0], config: body.config, revision: rules[0].revision + 1, pending: false }
        return fulfill(route, rules[0])
      }
      if (url.pathname.endsWith('/delete')) {
        rules = []
        return fulfill(route, null)
      }
      return route.abort()
    })
    await page.goto(`http://127.0.0.1:${port}/quality-ops`)
    await page.getByRole('heading', { name: '质量运维', exact: true }).waitFor()
    await page.getByRole('button', { name: '查看结果', exact: true }).waitFor()
    await page.getByRole('button', { name: '立即检测', exact: true }).dblclick()
    assert.equal(enqueues, 1)
    await page.getByRole('button', { name: '暂停定时检测', exact: true }).click()
    await page.getByRole('button', { name: '启用定时检测', exact: true }).waitFor()
    assert.equal(saves, 1)
    assert.match(await page.locator('.quality-rule').getByText('已暂停', { exact: true }).getAttribute('class'), /text-cp-text-secondary/)
    await page.getByRole('button', { name: '编辑规则', exact: true }).click()
    const editor = page.getByRole('dialog', { name: '编辑检测规则' })
    await editor.waitFor()
    await editor.getByRole('textbox', { name: /^题目/ }).fill('Changed fixture question')
    await editor.getByRole('button', { name: '保存', exact: true }).click()
    await editor.waitFor({ state: 'hidden' })
    assert.equal(rules[0].config.prompt, 'Changed fixture question')
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 })
      await page.mouse.move(0, 0)
      await page.screenshot({ path: `${output}/quality-${width}.png`, fullPage: true, animations: 'disabled' })
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1))
      if (width < 640) {
        const result = page.getByRole('article')
        assert.equal(await result.count(), 1)
        assert.ok(await result.getByText('通过', { exact: true }).first().isVisible())
        assert.ok(await result.getByText('错误', { exact: true }).isVisible())
        const resultBox = await result.boundingBox()
        assert.ok(resultBox && resultBox.x >= 0 && resultBox.x + resultBox.width <= width + 1)
      }
      await page.getByRole('button', { name: '查看结果', exact: true }).click()
      const drawer = page.getByRole('dialog', { name: '检测详情' })
      await drawer.getByText('fixture comparison').waitFor()
      assert.equal(await page.evaluate(() => window.__qualityExecuted), undefined)
      const box = await drawer.boundingBox()
      assert.ok(box && box.x >= 0 && box.x + box.width <= width + 1)
      await page.screenshot({ path: `${output}/quality-detail-${width}.png`, fullPage: true, animations: 'disabled' })
      await drawer.getByRole('button', { name: '关闭', exact: true }).click()
      await page.getByRole('button', { name: '编辑规则', exact: true }).click()
      await editor.waitFor()
      await page.screenshot({ path: `${output}/quality-editor-${width}.png`, fullPage: true, animations: 'disabled' })
      assert.ok(await editor.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
      const saveBox = await editor.getByRole('button', { name: '保存', exact: true }).boundingBox()
      assert.ok(saveBox && saveBox.y >= 0 && saveBox.y + saveBox.height <= 1000)
      await editor.getByRole('button', { name: '取消', exact: true }).click()
    }
    await page.setViewportSize({ width: 1440, height: 1000 })
    await page.getByRole('button', { name: '切换暗黑模式', exact: true }).click()
    await page.getByRole('button', { name: '切换浅色模式', exact: true }).waitFor()
    await page.mouse.move(0, 0)
    await page.screenshot({ path: `${output}/quality-dark.png`, fullPage: true, animations: 'disabled' })
    await page.getByRole('button', { name: '编辑规则', exact: true }).click()
    await editor.waitFor()
    await page.screenshot({ path: `${output}/quality-editor-dark.png`, fullPage: true, animations: 'disabled' })
    await editor.getByRole('button', { name: '取消', exact: true }).click()
    failHistory = true
    await page.getByRole('button', { name: '刷新', exact: true }).click()
    await page.getByRole('alert').waitFor()
    assert.equal(await page.getByRole('button', { name: '查看结果', exact: true }).count(), 1)
    failHistory = false
    await page.getByRole('button', { name: '刷新', exact: true }).click()
    await page.getByRole('alert').waitFor({ state: 'hidden' })
    await page.getByRole('button', { name: '删除规则', exact: true }).click()
    await page.getByRole('alertdialog').getByRole('button', { name: '确认', exact: true }).click()
    await page.getByText('暂无规则', { exact: true }).waitFor()
    assert.equal(rules.length, 0)
    await page.getByRole('button', { name: '新建规则', exact: true }).click()
    const creation = page.getByRole('dialog', { name: '新建检测规则' })
    await creation.getByRole('combobox', { name: '被测账号', exact: true }).click()
    await page.getByRole('option', { name: /fixture-quality-account/ }).click()
    await creation.getByRole('textbox', { name: /^检测模型/ }).fill('fixture-create-model')
    await creation.getByRole('textbox', { name: /^题目/ }).fill('question')
    await creation.getByRole('textbox', { name: /^参考答案/ }).fill('answer')
    await creation.getByRole('combobox', { name: '判题分组', exact: true }).click()
    await page.getByRole('option', { name: '独立判题分组', exact: true }).click()
    await creation.getByRole('textbox', { name: /^判题模型/ }).fill('fixture-judge')
    await creation.getByRole('combobox', { name: '推理强度', exact: true }).click()
    await page.getByRole('option', { name: 'high', exact: true }).click()
    await creation.getByRole('button', { name: '保存', exact: true }).click()
    await creation.waitFor({ state: 'hidden' })
    assert.equal(rules[0].config.accountId, config.accountId)
    assert.equal(rules[0].config.reasoningEffort, 'high')
    assert.deepEqual(errors, [])
    process.stdout.write('Quality UI: scheduling controls, revision payload, deduplication, escaped detail and 1440/390/320px layouts passed.\n')
  }
  finally {
    await browser?.close()
    if (server.exitCode === null) {
      const stopped = new Promise(resolve => server.once('exit', resolve))
      server.kill('SIGTERM')
      await stopped
    }
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
