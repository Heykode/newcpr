import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'

const entry = `
import {createApp,h,reactive,ref,nextTick} from 'vue'
import Form from '/src/components/base/BaseForm/index.vue'
import Item from '/src/components/base/BaseForm/FormItem.vue'
import Input from '/src/components/base/BaseInput.vue'
import NumberInput from '/src/components/base/BaseNumberInput.vue'
import Group from '/src/views/account-groups/components/AccountGroupFormModal.vue'
import Proxy from '/src/views/proxies/components/ProxyFormModal.vue'
import Health from '/src/views/dashboard/components/RequestHealthTimelineCard.vue'
import Markdown from '/src/components/base/BaseMarkdown/index.vue'
import {resolveTheme,applyResolvedTheme} from '/src/theme/index.ts'
import '/src/styles/index.css'
import {setLocale} from '/src/i18n/index.ts'
await setLocale('zh-Hans')
const form=ref(), draft=reactive({name:'',count:1,async:'',pair:''})
const group=reactive({name:'',color:'#112233',description:'',enabled:true,fastMode:'default',allowImageGeneration:true})
const open=ref(false),proxyOpen=ref(false), proxy=reactive({name:'',proxyUrl:'',maxWebsocketConnections:0,autoLocation:false,locationEnabled:false,location:{country:'',region:'',city:'',timezone:''}})
window.qa={draft,open,proxyOpen,group,proxy,saves:0,valid:false}
window.theme=scheme=>applyResolvedTheme(document.documentElement,resolveTheme(scheme,'relay-blue','#1677ff'))
window.theme('light')
const rules={name:{required:true,whitespace:true,message:'名称不能为空',trigger:['blur','change']},count:{type:'integer',min:1,max:3,message:'数值范围1至3',trigger:'change'},async:{asyncValidator:(_rule,value)=>new Promise((resolve,reject)=>setTimeout(()=>value==='bad'?reject(new Error('过时错误')):resolve(),value==='bad'?120:5)),trigger:'change'}}
createApp({render:()=>h('main',{style:'padding:16px;max-width:900px;margin:auto'},[
 h(Form,{ref:form,model:draft,rules,scrollToError:true,focusOnError:true,novalidate:true},{default:()=>[
   h(Form,{}, {default:()=>h(Item,{prop:'name',label:'名称'},()=>h(Input,{'modelValue':draft.name,'onUpdate:modelValue':v=>draft.name=v,'aria-label':'名称'}))}),
   h(Item,{prop:'count',label:'数量'},()=>h(NumberInput,{modelValue:draft.count,'onUpdate:modelValue':v=>draft.count=v,label:'数量'})),
   h(Item,{prop:'async',label:'异步字段'},()=>h(Input,{modelValue:draft.async,'onUpdate:modelValue':v=>draft.async=v,'aria-label':'异步字段'})),
   h(Item,{label:'复合字段'},()=>[h(Input,{'aria-label':'复合一',modelValue:''}),h(Input,{'aria-label':'复合二',modelValue:''})]),
   h('button',{type:'button',onClick:async()=>window.qa.valid=await form.value.validate().catch(()=>false)},'校验'),
 ]}),
 h('button',{onClick:()=>open.value=true},'打开分组'),
 h('button',{onClick:()=>proxyOpen.value=true},'打开代理'),
 h(Group,{modelValue:open.value,'onUpdate:modelValue':v=>open.value=v,form:group,'onUpdate:form':v=>Object.assign(group,v),group:null,saving:false,onSave:()=>window.qa.saves++}),
 h(Proxy,{modelValue:proxyOpen.value,'onUpdate:modelValue':v=>proxyOpen.value=v,proxy:null,saving:false,testing:false,...proxy,...Object.fromEntries(Object.keys(proxy).map(k=>['onUpdate:'+k,v=>proxy[k]=v])),onSave:()=>window.qa.saves++}),
 h(Health,{timeline:{title:'请求健康度',description:'',points:[],reliabilityDisplay:'--',successRequests:0,failedRequests:0,incompleteRequests:0}}),
 h(Markdown,{class:'release-notes',source:('> [!WARNING]\\n> 发布提示\\n\\n<script>alert(1)</script>')})
])}).mount('#app')
await nextTick()
window.qa.form=form.value
`

async function main() {
  const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href)
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    server: { host: '127.0.0.1', port: 0 },
    plugins: [{
      name: 'isolated-form-qa',
      configResolved(config) { config.server.proxy = {} },
      resolveId(id) { return id === '/v3372-qa.js' ? '\0v3372-qa' : undefined },
      load(id) { return id === '\0v3372-qa' ? entry : undefined },
    }],
  })
  await server.listen()
  const base = `http://127.0.0.1:${server.httpServer.address().port}`
  const output = process.env.QA_OUTPUT_DIR
  await mkdir(output, { recursive: true })
  const browser = await chromium.launch({ executablePath: process.env.CHROME_PATH || undefined, headless: true })
  try {
    for (const width of [1440, 390, 320]) {
      const page = await browser.newPage({ viewport: { width, height: 1000 }, reducedMotion: 'reduce' })
      const errors = []
      page.on('pageerror', error => errors.push(error.message))
      await page.route('**/*', async (route) => {
        const url = new URL(route.request().url())
        assert.equal(url.origin, base, 'test must not access production')
        if (url.pathname === '/qa')
          return route.fulfill({ contentType: 'text/html', body: '<html><body><div id="app"></div><script type="module" src="/v3372-qa.js"></script></body></html>' })
        return route.continue()
      })
      await page.goto(`${base}/qa`)
      await page.getByRole('button', { name: '校验', exact: true }).click()
      await page.getByText('名称不能为空', { exact: true }).waitFor()
      assert.equal(await page.evaluate(() => document.activeElement.getAttribute('aria-label')), '名称')
      assert.equal(await page.evaluate(() => window.qa.valid), false)
      await page.getByRole('textbox', { name: '名称', exact: true }).fill('有效名称')
      await page.getByRole('button', { name: '校验', exact: true }).click()
      await page.waitForFunction(() => window.qa.valid)
      const ids = await page.locator('input').evaluateAll(inputs => inputs.map(input => input.id))
      assert.equal(new Set(ids).size, ids.length)
      await page.getByRole('textbox', { name: '异步字段' }).fill('bad')
      await page.getByRole('textbox', { name: '异步字段' }).fill('good')
      await page.waitForTimeout(180)
      assert.equal(await page.getByText('过时错误', { exact: true }).count(), 0)
      await page.getByRole('spinbutton', { name: '数量', exact: true }).fill('4')
      await page.getByRole('button', { name: '校验', exact: true }).click()
      await page.getByText('数值范围1至3').waitFor()
      await page.waitForFunction(() => document.activeElement.getAttribute('aria-label') === '数量')
      assert.equal(await page.evaluate(() => document.activeElement.getAttribute('aria-label')), '数量')
      await page.getByRole('button', { name: '打开分组' }).click()
      await page.getByRole('button', { name: '保存分组' }).click()
      await page.getByText('请输入分组名称', { exact: true }).waitFor()
      assert.equal(await page.evaluate(() => window.qa.saves), 0)
      await page.getByRole('textbox', { name: '分组名称', exact: true }).fill('测试分组')
      await page.getByRole('button', { name: '保存分组' }).click()
      await page.waitForFunction(() => window.qa.saves === 1)
      await page.getByRole('button', { name: '取消', exact: true }).click()
      await page.getByRole('button', { name: '打开代理' }).click()
      await page.getByRole('button', { name: '保存代理', exact: true }).click()
      await page.getByText('请输入代理名称', { exact: true }).waitFor()
      assert.equal(await page.evaluate(() => window.qa.saves), 1)
      await page.getByRole('button', { name: '取消', exact: true }).click()
      assert.equal(await page.locator('[aria-label="暂无请求数据"] > span').count(), 96)
      assert.equal(await page.locator('.markdown-alert-warning').count(), 1)
      assert.equal(await page.locator('.release-notes script').count(), 0)
      for (const scheme of ['light', 'dark']) {
        await page.evaluate(value => window.theme(value), scheme)
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth), false)
        await page.screenshot({ path: `${output}/forms-${width}-${scheme}.png`, fullPage: true })
      }
      assert.deepEqual(errors, [])
      await page.close()
    }
  }
  finally {
    await browser.close()
    await server.close()
  }
  console.warn('v3.37.2 form validation, stale results, native modals, empty timeline, Markdown and responsive themes passed')
}
main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
