import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'

const entry = `
import {createApp,h,ref} from 'vue'
import Form from '/src/components/base/BaseForm/index.vue'
import Warmup from '/src/views/settings/components/AccountWarmupCard.vue'
import Select from '/src/components/base/BaseSelect.vue'
import Tags from '/src/components/base/BaseInputTag.vue'
import Input from '/src/components/base/BaseInput.vue'
import {useSettingsForm} from '/src/views/settings/composables/useSettingsForm.ts'
import {useSettingsFormRules} from '/src/views/settings/composables/useSettingsFormRules.ts'
import {resolveTheme,applyResolvedTheme} from '/src/theme/index.ts'
import {setLocale} from '/src/i18n/index.ts'
import '/src/styles/index.css'
await setLocale('zh-Hans')
applyResolvedTheme(document.documentElement,resolveTheme('light','deep-teal','#1677ff'))
createApp({setup(){
 const state=useSettingsForm(), form=ref(), rules=useSettingsFormRules(state.form)
 const selected=ref(1), multiple=ref([1,2,3]), disabled=ref(false), tags=ref(['one','two','three']), autofill=ref('')
 const options=[{value:1,label:'One'},{value:2,label:'Two'},{value:3,label:'Three'},{value:4,label:'Disabled',disabled:true}]
 window.qa={state,selected,multiple,disabled,tags,autofill}
 void state.loadSettings()
 return ()=>h('main',{style:'padding:12px;max-width:720px;margin:auto'},[
  h(Form,{ref:form,as:'div',model:state.form,rules:rules.value,validateOnRuleChange:false},()=>[
   h(Warmup,{enabled:state.form.accountWarmupEnabled,'onUpdate:enabled':v=>state.form.accountWarmupEnabled=v,cron:state.form.accountWarmupCron,'onUpdate:cron':v=>state.form.accountWarmupCron=v,model:state.form.accountWarmupModel,'onUpdate:model':v=>state.form.accountWarmupModel=v}),
   h('button',{onClick:async()=>{if(await form.value.validate().catch(()=>false))await state.saveSettings()}},'保存规则'),
   h('button',{onClick:()=>state.loadSettings()},'重读规则')
  ]),
  h(Select,{'aria-label':'数字单选',modelValue:selected.value,'onUpdate:modelValue':v=>selected.value=v,options,filterable:true,clearable:true,disabled:disabled.value}),
  h(Select,{'aria-label':'数字多选',modelValue:multiple.value,'onUpdate:modelValue':v=>multiple.value=v,options,multiple:true,collapseTags:true,collapseTagsTooltip:true,maxCollapseTags:1}),
  h(Input,{'aria-label':'自动填充',modelValue:autofill.value,'onUpdate:modelValue':v=>autofill.value=v,showPassword:true,clearable:true}),
  h(Tags,{'aria-label':'拖动标签',modelValue:tags.value,'onUpdate:modelValue':v=>tags.value=v,draggable:true}),
 ])
}}).mount('#app')
`

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    server: { host: '127.0.0.1', port: 0 },
    plugins: [{
      name: 'isolated-warmup-rules',
      configResolved(config) { config.server.proxy = {} },
      resolveId(id) { return id === '/warmup-qa.js' ? '\0warmup-qa' : undefined },
      load(id) { return id === '\0warmup-qa' ? entry : undefined },
    }],
  })
  await server.listen()
  let browser
  try {
    browser = await chromium.launch({ headless: true, ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) })
    const origin = `http://127.0.0.1:${server.httpServer.address().port}`
    for (const width of [1440, 390, 320]) {
      const page = await browser.newPage({ viewport: { width, height: 1000 }, reducedMotion: 'reduce' })
      const errors = []
      page.on('pageerror', error => errors.push(error.message))
      let saved = { configRevision: 1, refreshMarginSeconds: 1800, refreshConcurrency: 4, maxConcurrentPerAccount: 5, requestIntervalMs: 25, rotationStrategy: 'smart', modelMappings: {}, accountWarmupEnabled: true, accountWarmupCron: '30 1 * * *', accountWarmupModel: 'fixture-model' }
      const writes = []
      await page.route('**/*', (route) => {
        const url = new URL(route.request().url())
        assert.equal(url.origin, origin, 'isolated test never accesses external services')
        if (url.pathname === '/qa')
          return route.fulfill({ contentType: 'text/html', body: '<html><body><div id="app"></div><script type="module" src="/warmup-qa.js"></script></body></html>' })
        if (/^\/(?:dev\/)?api\//u.test(url.pathname)) {
          assert.match(url.pathname, /\/api\/admin\/settings(?:\/update)?$/u)
          if (route.request().method() === 'POST') {
            const payload = route.request().postDataJSON()
            writes.push(payload)
            saved = { ...payload, configRevision: saved.configRevision + 1 }
          }
          return route.fulfill({ contentType: 'application/json', body: JSON.stringify({ code: 200, message: 'ok', data: saved }) })
        }
        return route.continue()
      })
      await page.goto(`${origin}/qa`)
      const input = page.getByRole('textbox', { name: '预激活 Cron 表达式' })
      await input.waitFor()
      await page.waitForFunction(() => window.qa.state.form.accountWarmupCron.length === 1)
      const autofill = page.getByLabel('自动填充', { exact: true })
      await autofill.evaluate((element) => {
        const animation = [...document.styleSheets].flatMap(sheet => [...sheet.cssRules]).find(rule => rule instanceof CSSKeyframesRule && rule.name.includes('input-autofill-sync'))
        if (!animation)
          throw new Error('Input autofill animation is missing')
        element.value = 'synthetic-autofill'
        element.style.animation = `${animation.name} 1ms`
      })
      await page.waitForFunction(() => window.qa.autofill.value === 'synthetic-autofill', undefined, { timeout: 5000 })
      await page.getByRole('button', { name: '显示自动填充', exact: true }).click()
      assert.equal(await autofill.getAttribute('type'), 'text')
      await autofill.focus()
      await page.getByRole('button', { name: '清空输入', exact: true }).click()
      assert.equal(await page.evaluate(() => window.qa.autofill.value), '')
      await input.fill('45 8 * * *')
      await input.press('Enter')
      await input.fill('0 0 * * MON,FRI')
      await page.getByRole('button', { name: '保存规则' }).click()
      await page.waitForFunction(() => !window.qa.state.saving.value)
      assert.equal(writes.at(-1)?.accountWarmupCron, '30 1 * * *;45 8 * * *;0 0 * * MON,FRI', 'blur commits the final rule before save')
      await page.getByRole('button', { name: '重读规则' }).click()
      await page.waitForFunction(() => window.qa.state.form.accountWarmupCron.length === 3)
      const more = page.getByRole('button', { name: /查看其余 \d+ 个标签/u })
      await more.waitFor()
      await more.press('Enter')
      const dialog = page.getByRole('dialog', { name: '其余标签' })
      await dialog.waitFor()
      await dialog.getByRole('button').last().click()
      assert.equal(await page.evaluate(() => window.qa.state.form.accountWarmupCron.length), 2)
      await input.click()
      await input.fill('0 9 * * *')
      await input.dispatchEvent('keydown', { key: 'Enter', code: 'Enter', isComposing: true, keyCode: 229 })
      assert.equal(await page.evaluate(() => window.qa.state.form.accountWarmupCron.length), 2, 'IME confirmation does not submit a tag')
      await input.press('Enter')
      await input.press('Backspace')
      assert.equal(await page.evaluate(() => window.qa.state.form.accountWarmupCron.length), 2)
      const count = writes.length
      await input.fill('invalid')
      await input.press('Enter')
      await page.getByRole('button', { name: '保存规则' }).click()
      await page.getByText('请输入有效的 Cron 表达式', { exact: true }).waitFor()
      assert.equal(writes.length, count)
      await input.press('Backspace')
      const select = page.getByRole('combobox', { name: '数字单选' })
      await select.fill('Two')
      await page.getByRole('option', { name: 'Two', exact: true }).click()
      assert.equal(await page.evaluate(() => window.qa.selected.value), 2)
      await select.focus()
      await page.getByRole('button', { name: '清空选择', exact: true }).click()
      assert.equal(await page.evaluate(() => window.qa.selected.value), undefined)
      await select.press('ArrowDown')
      await select.press('ArrowDown')
      await select.press('Enter')
      assert.equal(await page.evaluate(() => window.qa.selected.value), 2)
      await page.evaluate(() => window.qa.disabled.value = true)
      assert.equal(await select.isDisabled(), true)
      await page.getByRole('button', { name: /查看其余 \d+ 个已选项/u }).press('Enter')
      const remaining = page.getByRole('dialog', { name: '其余已选项' })
      await remaining.waitFor()
      await remaining.getByRole('button', { name: '移除 Two' }).click()
      assert.deepEqual(await page.evaluate(() => window.qa.multiple.value), [1, 3])
      await remaining.press('Escape')
      await remaining.waitFor({ state: 'hidden' })
      await page.getByRole('textbox', { name: '拖动标签' }).focus()
      const tags = page.locator('[data-input-tag]').filter({ hasText: /^(?:one|two|three)移除$/u })
      const first = await tags.first().boundingBox()
      const last = await tags.last().boundingBox()
      assert(first && last)
      await page.mouse.move(first.x + 10, first.y + first.height / 2)
      await page.mouse.down()
      await page.mouse.move(first.x + 16, first.y + first.height / 2, { steps: 4 })
      await page.waitForTimeout(150)
      await page.mouse.move(last.x + last.width / 2, last.y + last.height / 2, { steps: 15 })
      await page.waitForTimeout(150)
      await page.mouse.up()
      await page.waitForFunction(() => window.qa.tags.value[0] !== 'one')
      assert.deepEqual([...await page.evaluate(() => window.qa.tags.value)].sort(), ['one', 'three', 'two'])
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1), false)
      if (process.env.QA_OUTPUT_DIR) {
        await mkdir(process.env.QA_OUTPUT_DIR, { recursive: true })
        await page.screenshot({ path: `${process.env.QA_OUTPUT_DIR}/warmup-${width}.png`, fullPage: true })
      }
      assert.deepEqual(errors, [])
      await page.close()
    }
    console.warn('Warmup save/reload, tags, IME, drag, overflow, numeric selection and mobile layout passed.')
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
