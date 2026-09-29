import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { CANDY_PROMPT, CANDY_REFERENCE_ANSWER, DEFAULT_JUDGE_PROMPT } from '../../src/views/quality-ops/presets.ts'

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
    browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH || undefined })
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
    const errors = []
    page.on('pageerror', error => errors.push(error.message))
    const fulfill = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
    const now = new Date().toISOString()
    const template = {
      id: 'quality-template',
      revision: 2,
      config: {
        name: 'Excel 异常处置',
        enabled: true,
        concurrencyLimit: 5,
        weight: 20,
        groupIds: ['quality-group'],
        outboundProxyId: null,
        responsesUpstream: 'excel',
        excelModelsFollowGlobal: true,
        excelCacheCreationAsInput: true,
        excelIgnoreEncryptedContent: true,
        excel403Action: 'disable_excel',
        egressMode: 'random_ipv6_reuse',
      },
    }
    await page.route('**/dev/api/admin/relogin/templates', route => fulfill(route, [template]))
    await page.route('**/dev/api/admin/proxies?**', route => fulfill(route, {
      items: [],
      page: { page: 1, pageSize: 50, total: 0, totalPages: 0 },
    }))
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
      failureAction: 'none',
      failureGroupIds: [],
      autoRestore: false,
    }
    let rules = [{ id: 'quality-rule', revision: 1, config, nextRunAt: now, running: false, pending: false, lastStatus: 'correct', lastRunAt: now }]
    const run = { id: 'quality-run', ruleId: 'quality-rule', accountId: config.accountId, model: config.model, status: 'correct', startedAt: now, finishedAt: now, correct: 2, incorrect: 0, unknown: 0, requestErrors: 0 }
    let enqueues = 0
    let saves = 0
    let failHistory = false
    let failGroups = false
    let failAccounts = false
    let failCreationAccount = ''
    let failRules = false
    let failEditId = ''
    const editRequests = []
    let groupRequests = 0
    const catalogAccounts = [
      { id: config.accountId, name: 'fixture-quality-account@example.test' },
      ...Array.from({ length: 60 }, (_, i) => ({ id: `sample-${i + 1}`, name: `sample-${String(i + 1).padStart(2, '0')}@example.test` })),
    ]
    await page.route('**/dev/api/admin/accounts?**', async (route) => {
      if (failAccounts)
        return route.fulfill({ status: 503, json: { code: 503, message: 'fixture accounts unavailable', data: null } })
      const params = new URL(route.request().url()).searchParams
      const search = params.get('search') || ''
      const current = Number(params.get('page') || 1)
      const size = Number(params.get('pageSize') || 50)
      const items = catalogAccounts.filter(account => account.name.includes(search))
      if (search === 'sample-01')
        await new Promise(resolve => setTimeout(resolve, 500))
      return fulfill(route, { items: items.slice((current - 1) * size, current * size), page: { page: current, pageSize: size, total: items.length, totalPages: Math.ceil(items.length / size) } })
    })
    await page.route('**/dev/api/admin/account-groups?**', (route) => {
      groupRequests++
      assert.ok([null, 'true'].includes(new URL(route.request().url()).searchParams.get('enabled')))
      return failGroups
        ? route.fulfill({ status: 503, json: { code: 503, message: 'fixture groups unavailable', data: null } })
        : fulfill(route, {
            items: [{ id: 'quality-group', name: '独立判题分组', enabled: true, memberCount: 3 }],
            page: { page: 1, pageSize: 50, total: 1, totalPages: 1 },
          })
    })
    await page.route('**/dev/api/admin/quality-ops/**', async (route) => {
      const url = new URL(route.request().url())
      if (url.pathname.endsWith('/rules') && failRules)
        return route.fulfill({ status: 503, json: { code: 503, message: 'fixture rules unavailable', data: null } })
      if (url.pathname.endsWith('/rules'))
        return fulfill(route, rules)
      if (url.pathname.endsWith('/runs') && failHistory)
        return route.fulfill({ status: 503, json: { code: 503, message: 'fixture history unavailable', data: null } })
      if (url.pathname.endsWith('/runs'))
        return fulfill(route, [run])
      if (url.pathname.endsWith('/detail')) {
        return fulfill(route, { ...run, answers: [{
          ...(run.detectionMode === 'state_probe'
            ? { probe: {
                verdict: 'degraded',
                reason: 'changed',
                shots: [
                  { transport: 'http_sse', status: 200, ticketLength: 332, changed: null, reason: null },
                  { transport: 'http_sse', status: 200, ticketLength: 332, changed: true, reason: null },
                ],
              } }
            : {}),
          index: 1,
          answer: '<script>window.__qualityExecuted = true</script>',
          verdict: run.detectionMode === 'state_probe' ? 'incorrect' : 'correct',
          reason: run.detectionMode === 'state_probe' ? '观察到异常换票' : 'fixture comparison',
          elapsedMs: 1350,
          returnedModel: 'fixture-model',
          judgeAccountId: 'judge-account',
        }] })
      }
      if (url.pathname.endsWith('/run')) {
        enqueues++
        assert.deepEqual(route.request().postDataJSON(), { id: rules[0].id, revision: rules[0].revision })
        assert.equal(rules[0].pending, false)
        assert.equal(rules[0].running, false)
        rules[0].pending = true
        return fulfill(route, null)
      }
      if (url.pathname.endsWith('/save')) {
        saves++
        const body = route.request().postDataJSON()
        if (body.id === null) {
          assert.equal(body.revision, null)
          if (body.config.accountId === failCreationAccount)
            return route.fulfill({ status: 503, json: { code: 503, message: 'fixture save failed', data: null } })
          assert.ok(!rules.some(rule => rule.config.accountId === body.config.accountId))
          const result = { id: `new-${body.config.accountId}`, revision: 1, config: body.config, nextRunAt: now, running: false, pending: false, lastStatus: null, lastRunAt: null }
          rules.push(result)
          return fulfill(route, result)
        }
        editRequests.push(body)
        if (body.id === failEditId)
          return route.fulfill({ status: 503, json: { code: 503, message: 'fixture edit failed', data: null } })
        const index = rules.findIndex(rule => rule.id === body.id)
        assert.ok(index >= 0)
        assert.equal(body.config.accountId, rules[index].config.accountId)
        assert.equal(body.revision, rules[index].revision)
        rules[index] = { ...rules[index], config: body.config, revision: rules[index].revision + 1, pending: false, excelFailureStreak: 0 }
        return fulfill(route, rules[index])
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
    assert.equal(await page.getByRole('button', { name: '立即检测', exact: true }).isDisabled(), false)
    assert.equal(saves, 1)
    assert.match(await page.locator('.quality-rule').getByText('已暂停', { exact: true }).getAttribute('class'), /text-cp-text-secondary/)
    await page.getByRole('button', { name: '立即检测', exact: true }).dblclick()
    await page.locator('.quality-rule').getByText('已排队', { exact: true }).waitFor()
    assert.equal(enqueues, 2)
    assert.equal(saves, 1)
    assert.equal(rules[0].config.enabled, false)
    assert.equal(await page.getByRole('button', { name: '立即检测', exact: true }).isDisabled(), true)
    rules[0].pending = false
    rules[0].running = true
    await page.reload()
    await page.locator('.quality-rule').getByText('检测中', { exact: true }).waitFor()
    assert.equal(await page.getByRole('button', { name: '立即检测', exact: true }).isDisabled(), true)
    rules[0].running = false
    await page.reload()
    await page.locator('.quality-rule').getByText('已暂停', { exact: true }).waitFor()
    assert.equal(await page.getByRole('button', { name: '立即检测', exact: true }).isDisabled(), false)
    assert.equal(saves, 1)
    await page.getByRole('button', { name: '编辑规则', exact: true }).click()
    const editor = page.getByRole('dialog', { name: '编辑检测规则' })
    await editor.waitFor()
    assert.equal(await editor.getByRole('textbox', { name: /^题目/ }).inputValue(), config.prompt)
    assert.equal(await editor.getByRole('textbox', { name: /^参考答案/ }).inputValue(), config.referenceAnswer)
    assert.equal(await editor.getByRole('textbox', { name: /^判题提示词/ }).inputValue(), config.judgePrompt)
    await editor.getByText('每天 00:00、06:00、12:00、18:00（所选时区）', { exact: true }).waitFor()
    await editor.getByRole('combobox', { name: /^检测频率/ }).click()
    await page.getByRole('option', { name: '自定义 Cron（高级）', exact: true }).click()
    await editor.getByRole('textbox', { name: /^Cron 表达式/ }).fill('15 9 * * 1-5')
    await editor.getByRole('textbox', { name: /^题目/ }).fill('Changed fixture question')
    await editor.getByRole('button', { name: '保存', exact: true }).click()
    await editor.waitFor({ state: 'hidden' })
    assert.equal(rules[0].config.prompt, 'Changed fixture question')
    assert.equal(rules[0].config.cron, '15 9 * * 1-5')
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
      assert.equal(await editor.getByRole('textbox', { name: /^Cron 表达式/ }).inputValue(), '15 9 * * 1-5')
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
    failGroups = true
    await page.getByRole('button', { name: '新建规则', exact: true }).click()
    const creation = page.getByRole('dialog', { name: '新建检测规则' })
    assert.equal(await creation.getByRole('textbox', { name: /^题目/ }).inputValue(), CANDY_PROMPT)
    assert.equal(await creation.getByRole('textbox', { name: /^参考答案/ }).inputValue(), CANDY_REFERENCE_ANSWER)
    assert.equal(await creation.getByRole('textbox', { name: /^判题提示词/ }).inputValue(), DEFAULT_JUDGE_PROMPT)
    const accountPicker = creation.getByRole('group', { name: /^被测账号/ })
    await accountPicker.getByRole('checkbox', { name: 'fixture-quality-account@example.test', exact: true }).check()
    await creation.getByText('fixture groups unavailable', { exact: true }).waitFor()
    failGroups = false
    await creation.getByRole('button', { name: '重试加载判题账号分组', exact: true }).click()
    await creation.getByRole('radio', { name: '独立判题分组', exact: true }).waitFor()
    const originalGroupRequests = groupRequests
    await accountPicker.getByRole('button', { name: '加载更多被测账号', exact: true }).click()
    await accountPicker.getByRole('checkbox', { name: 'sample-60@example.test', exact: true }).check()
    const search = accountPicker.getByRole('textbox', { name: '搜索被测账号', exact: true })
    const slowSearch = page.waitForRequest(request => request.url().includes('/accounts?') && new URL(request.url()).searchParams.get('search') === 'sample-01')
    await search.fill('sample-01')
    await slowSearch
    await search.fill('sample-02')
    await accountPicker.getByRole('checkbox', { name: 'sample-02@example.test', exact: true }).waitFor()
    assert.equal(await accountPicker.getByRole('checkbox').count(), 1)
    assert.equal(await accountPicker.getByRole('checkbox', { name: 'sample-01@example.test', exact: true }).count(), 0)
    await accountPicker.getByText('已选 2 项', { exact: true }).waitFor()
    await search.fill('no-such-account')
    await accountPicker.getByText('没有匹配结果', { exact: true }).waitFor()
    await search.fill('')
    await accountPicker.getByRole('checkbox', { name: 'fixture-quality-account@example.test', exact: true }).check()
    assert.equal(groupRequests, originalGroupRequests)
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 })
      await accountPicker.scrollIntoViewIfNeeded()
      assert.ok(await creation.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
      const list = accountPicker.locator('[aria-label="被测账号列表"]')
      assert.ok(await list.evaluate(element => element.scrollHeight > element.clientHeight && element.clientHeight <= 210))
      await page.screenshot({ path: `${output}/quality-create-${width}.png`, fullPage: true, animations: 'disabled' })
    }
    await page.setViewportSize({ width: 1440, height: 1000 })
    await creation.getByRole('textbox', { name: /^检测模型/ }).fill('fixture-create-model')
    await creation.getByRole('textbox', { name: /^题目/ }).fill('question')
    await creation.getByRole('textbox', { name: /^参考答案/ }).fill('answer')
    await creation.getByRole('radio', { name: '独立判题分组', exact: true }).check()
    await creation.getByRole('textbox', { name: /^判题模型/ }).fill('fixture-judge')
    await creation.getByRole('combobox', { name: '推理强度', exact: true }).click()
    await page.getByRole('option', { name: 'high', exact: true }).click()
    await creation.getByRole('combobox', { name: /^检测频率/ }).click()
    await page.getByRole('option', { name: '每天固定时间', exact: true }).click()
    await creation.getByLabel(/^每天检测时间/).fill('08:30')
    await creation.getByText('每天 08:30（所选时区）', { exact: true }).waitFor()
    await creation.getByRole('combobox', { name: '处理方式', exact: true }).click()
    await page.getByRole('option', { name: '移出指定分组', exact: true }).click()
    await creation.getByRole('group', { name: /^处置分组/ }).getByRole('checkbox', { name: '独立判题分组', exact: true }).check()
    await creation.getByText('后续整轮通过后自动恢复', { exact: true }).click()
    assert.equal(await creation.getByRole('switch', { name: '后续整轮通过后自动恢复', exact: true }).isChecked(), true)
    failCreationAccount = 'sample-60'
    await creation.getByRole('button', { name: '保存', exact: true }).click()
    await creation.getByText(/1 项未保存；成功项已保留/).waitFor()
    assert.equal(rules.length, 1)
    await accountPicker.getByText('已选 1 项', { exact: true }).waitFor()
    failCreationAccount = ''
    await creation.getByRole('button', { name: '保存', exact: true }).click()
    await creation.waitFor({ state: 'hidden' })
    assert.equal(rules.length, 2)
    assert.equal(rules[1].config.accountId, 'sample-60')
    assert.equal(rules[1].config.failureAction, 'remove_groups')
    assert.deepEqual(rules[1].config.failureGroupIds, ['quality-group'])
    assert.equal(rules[1].config.autoRestore, true)
    assert.equal(rules[0].config.accountId, config.accountId)
    assert.equal(rules[0].config.reasoningEffort, 'high')
    assert.equal(rules[0].config.cron, '30 8 * * *')
    failAccounts = true
    await page.getByRole('button', { name: '新建规则', exact: true }).click()
    await creation.getByText('fixture accounts unavailable', { exact: true }).waitFor()
    await creation.getByRole('radio', { name: '独立判题分组', exact: true }).check()
    failAccounts = false
    await creation.getByRole('button', { name: '重试加载被测账号', exact: true }).click()
    await accountPicker.getByRole('checkbox', { name: 'fixture-quality-account@example.test', exact: true }).waitFor()
    assert.equal(await search.inputValue(), '')
    await creation.getByRole('button', { name: '取消', exact: true }).click()
    await page.getByRole('button', { name: '新建规则', exact: true }).click()
    await creation.getByRole('textbox', { name: /^题目/ }).fill('Preserved question')
    await creation.getByRole('combobox', { name: /^检测模式/ }).click()
    await page.getByRole('option', { name: '状态探针', exact: true }).click()
    assert.equal(await creation.getByRole('textbox', { name: /^题目/ }).count(), 0)
    assert.equal(await creation.getByRole('textbox', { name: /^判题模型/ }).count(), 0)
    assert.equal(await creation.getByRole('spinbutton', { name: '每轮并行答题次数' }).count(), 0)
    await creation.getByRole('combobox', { name: /^检测模式/ }).click()
    await page.getByRole('option', { name: '题目检测', exact: true }).click()
    assert.equal(await creation.getByRole('textbox', { name: /^题目/ }).inputValue(), 'Preserved question')
    await creation.getByRole('combobox', { name: /^检测模式/ }).click()
    await page.getByRole('option', { name: '状态探针', exact: true }).click()
    await accountPicker.getByRole('checkbox', { name: 'sample-02@example.test', exact: true }).check()
    await creation.getByRole('textbox', { name: /^检测模型/ }).fill('fixture-model')
    await creation.getByRole('combobox', { name: '处理方式', exact: true }).click()
    assert.equal(await page.getByRole('option', { name: /^开启Excel模式/ }).count(), 0)
    await page.getByRole('option', { name: '应用账号模板', exact: true }).click()
    assert.equal(await creation.getByRole('button', { name: '保存', exact: true }).isDisabled(), true)
    await creation.getByRole('combobox', { name: '异常处置账号模板', exact: true }).click()
    await page.getByRole('option', { name: template.config.name, exact: true }).click()
    await creation.getByText('开启（有损省略）', { exact: true }).waitFor()
    await creation.getByText('关闭Excel模式', { exact: true }).waitFor()
    assert.equal(await creation.getByRole('switch', { name: '后续整轮通过后自动恢复', exact: true }).count(), 0)
    assert.equal(await creation.getByRole('spinbutton', { name: '连续异常阈值', exact: true }).inputValue(), '1')
    await creation.getByRole('spinbutton', { name: '连续异常阈值', exact: true }).fill('3')
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 })
      assert.ok(await creation.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
      await page.screenshot({ path: `${output}/quality-probe-editor-${width}.png`, fullPage: true, animations: 'disabled' })
      await creation.getByRole('spinbutton', { name: '连续异常阈值', exact: true }).scrollIntoViewIfNeeded()
      await page.screenshot({ path: `${output}/quality-threshold-${width}.png`, fullPage: true, animations: 'disabled' })
    }
    await creation.getByRole('button', { name: '保存', exact: true }).click()
    await creation.waitFor({ state: 'hidden' })
    assert.equal(rules[2].config.detectionMode, 'state_probe')
    assert.equal(rules[2].config.repetitions, 1)
    assert.equal(rules[2].config.failureAction, 'apply_account_template')
    assert.deepEqual(rules[2].config.failureTemplate, template)
    assert.equal(rules[2].config.autoRestore, false)
    assert.equal(rules[2].config.excelFailureThreshold, 3)
    Object.assign(run, { config: rules[2].config, detectionMode: 'state_probe', status: 'incorrect', correct: 0, incorrect: 1, action: 'template_applied_probe_paused' })
    rules[2].config.enabled = false
    rules[2].lastAction = 'template_applied_probe_paused'
    await page.getByRole('button', { name: '刷新', exact: true }).click()
    await page.getByText('已应用账号模板并暂停状态探针', { exact: true }).waitFor()
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 })
      await page.getByRole('button', { name: '查看结果', exact: true }).click()
      const drawer = page.getByRole('dialog', { name: '检测详情' })
      await drawer.getByText('本轮处置模板：Excel 异常处置（版本 2）', { exact: true }).waitFor()
      await drawer.getByText('票据已变化', { exact: true }).waitFor()
      assert.equal(await drawer.getByText('智商异常', { exact: true }).count(), 2)
      assert.ok(await drawer.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
      await page.screenshot({ path: `${output}/quality-probe-result-${width}.png`, fullPage: true, animations: 'disabled' })
      await drawer.getByRole('button', { name: '关闭', exact: true }).click()
    }
    await page.setViewportSize({ width: 1440, height: 1000 })
    await page.getByRole('button', { name: '切换浅色模式', exact: true }).click()
    await page.getByRole('checkbox', { name: '选择规则 fixture-quality-account@example.test', exact: true }).locator('..').click()
    assert.equal(await page.getByRole('checkbox', { name: '选择规则 fixture-quality-account@example.test', exact: true }).isChecked(), true)
    await page.getByRole('textbox', { name: '筛选规则', exact: true }).fill('sample-60')
    await page.getByRole('checkbox', { name: '全选搜索结果', exact: true }).locator('..').click()
    await page.getByRole('button', { name: '批量编辑（2）', exact: true }).waitFor()
    await page.getByRole('textbox', { name: '筛选规则', exact: true }).fill('')
    await page.getByRole('button', { name: '批量编辑（2）', exact: true }).click()
    const bulk = page.getByRole('dialog', { name: '批量编辑检测规则', exact: true })
    await bulk.waitFor()
    assert.equal(await bulk.locator('input[type=checkbox]:checked').count(), 0)
    assert.equal(await bulk.getByRole('button', { name: '保存所选字段', exact: true }).isDisabled(), true)
    await bulk.getByRole('checkbox', { name: '修改检测频率', exact: true }).locator('..').click()
    await bulk.getByRole('combobox', { name: /^检测频率/ }).click()
    await page.getByRole('option', { name: '自定义 Cron（高级）', exact: true }).click()
    await bulk.getByRole('textbox', { name: /^Cron 表达式/ }).fill('5 */2 * * *')
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 })
      assert.ok(await bulk.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
      const saveBox = await bulk.getByRole('button', { name: '保存所选字段', exact: true }).boundingBox()
      assert.ok(saveBox && saveBox.x >= 0 && saveBox.x + saveBox.width <= width + 1 && saveBox.y + saveBox.height <= 1000)
      await page.screenshot({ path: `${output}/quality-bulk-${width}.png`, fullPage: true, animations: 'disabled' })
    }
    const beforeBulk = editRequests.length
    failRules = true
    await bulk.getByRole('button', { name: '保存所选字段', exact: true }).click()
    await bulk.getByRole('alert').getByText('fixture rules unavailable', { exact: true }).waitFor()
    assert.equal(editRequests.length, beforeBulk)
    failRules = false
    rules[1].revision++
    rules[1].config.judgePrompt = 'Changed after the bulk dialog opened'
    failEditId = rules[1].id
    await bulk.getByRole('button', { name: '保存所选字段', exact: true }).dblclick()
    await bulk.getByRole('button', { name: '重试未完成项', exact: true }).waitFor()
    assert.equal(editRequests.length, beforeBulk + 2)
    assert.equal(rules[0].config.cron, '5 */2 * * *')
    assert.equal(rules[1].config.cron, '30 8 * * *')
    assert.equal(editRequests.at(-1).config.judgePrompt, 'Changed after the bulk dialog opened')
    failEditId = ''
    await bulk.getByRole('button', { name: '重试未完成项', exact: true }).click()
    await bulk.getByText(/待处理 0 条/).waitFor()
    assert.equal(editRequests.length, beforeBulk + 3)
    assert.equal(editRequests.at(-1).id, rules[1].id)
    assert.equal(rules[1].config.cron, '5 */2 * * *')
    assert.equal(rules[1].config.failureAction, 'remove_groups')
    assert.equal(rules[1].config.autoRestore, true)
    assert.equal(rules[2].config.excelFailureThreshold, 3)
    await bulk.getByRole('button', { name: '关闭', exact: true }).last().click()
    await page.getByRole('checkbox', { name: '选择规则 sample-02@example.test', exact: true }).locator('..').click()
    await page.getByRole('button', { name: '批量编辑（1）', exact: true }).click()
    assert.equal(await bulk.locator('input[type=checkbox]:checked').count(), 0)
    await bulk.getByRole('checkbox', { name: '修改连续异常阈值（模板或旧版开启 Excel 规则）', exact: true }).locator('..').click()
    await bulk.getByRole('spinbutton', { name: '连续异常阈值（模板或旧版开启 Excel 规则）', exact: true }).fill('5')
    await bulk.getByRole('checkbox', { name: '修改自动恢复（不适用于应用模板或旧版开启 Excel）', exact: true }).locator('..').click()
    await bulk.getByRole('switch', { name: '自动恢复（不适用于应用模板或旧版开启 Excel）', exact: true }).locator('..').click()
    assert.equal(await bulk.getByRole('switch', { name: '自动恢复（不适用于应用模板或旧版开启 Excel）', exact: true }).isChecked(), true)
    await bulk.getByRole('checkbox', { name: '修改异常处置账号模板（仅应用模板规则）', exact: true }).locator('..').click()
    await bulk.getByRole('combobox', { name: '异常处置账号模板', exact: true }).click()
    await page.getByRole('option', { name: template.config.name, exact: true }).click()
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 })
      await bulk.getByRole('spinbutton', { name: '连续异常阈值（模板或旧版开启 Excel 规则）', exact: true }).scrollIntoViewIfNeeded()
      assert.ok(await bulk.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
      await page.screenshot({ path: `${output}/quality-bulk-threshold-${width}.png`, fullPage: true, animations: 'disabled' })
    }
    await bulk.getByRole('button', { name: '保存所选字段', exact: true }).click()
    await bulk.getByText(/待处理 0 条/).waitFor()
    assert.equal(rules[2].config.excelFailureThreshold, 5)
    assert.equal(rules[2].config.autoRestore, false)
    assert.equal(rules[2].config.enabled, false)
    assert.deepEqual(rules[2].config.failureTemplate, template)
    await bulk.getByRole('button', { name: '关闭', exact: true }).last().click()
    // Selecting the same template name must adopt its new revision explicitly.
    template.revision++
    await page.setViewportSize({ width: 1440, height: 1000 })
    await page.locator('.quality-rule').filter({ hasText: 'sample-02@example.test' }).getByRole('button', { name: '编辑规则', exact: true }).click()
    await editor.getByRole('alert').getByText('所选模板已修改或删除，请重新选择并确认配置。', { exact: true }).waitFor()
    assert.equal(rules[2].config.failureTemplate.revision, 2)
    await editor.getByRole('combobox', { name: '异常处置账号模板', exact: true }).click()
    await page.getByRole('option', { name: template.config.name, exact: true }).click()
    await editor.getByRole('button', { name: '保存', exact: true }).click()
    await editor.waitFor({ state: 'hidden' })
    assert.equal(rules[2].config.failureTemplate.revision, 3)
    // An old direct-Excel rule remains editable without being silently converted.
    rules[2].config.failureAction = 'enable_excel'
    delete rules[2].config.failureTemplate
    await page.setViewportSize({ width: 1440, height: 1000 })
    await page.getByRole('button', { name: '刷新', exact: true }).click()
    await page.locator('.quality-rule').filter({ hasText: 'sample-02@example.test' }).getByRole('button', { name: '编辑规则', exact: true }).click()
    await editor.getByRole('combobox', { name: '处理方式', exact: true }).click()
    await page.getByRole('option', { name: '开启Excel模式（旧规则）', exact: true }).click()
    await editor.getByRole('button', { name: '保存', exact: true }).click()
    await editor.waitFor({ state: 'hidden' })
    assert.equal(rules[2].config.failureAction, 'enable_excel')
    assert.deepEqual(errors, [])
    process.stdout.write('Quality UI: legacy rules, probe modes, thresholds, selective bulk edits, refresh failure, partial retry, catalogs, schedules and 1440/390/320px layouts passed.\n')
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
