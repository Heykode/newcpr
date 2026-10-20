import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'

const root = fileURLToPath(new URL('../..', import.meta.url))
const entry = `
import {createApp,h} from 'vue'
import {createPinia} from 'pinia'
import Records from '/src/views/settings/components/backup/BackupRecordsCard.vue'
import '/src/styles/index.css'
createApp({render:()=>h('main',{style:'padding:12px;max-width:1100px;margin:auto'},[
h(Records,{records:[],page:1,pageSize:20,total:0,loading:false,error:'',activeBackup:false,creating:false,refreshing:false,deleting:false,deleteTarget:null,downloadStates:{},deleteOpen:false})
])}).use(createPinia()).mount('#app')
`

async function main() {
  const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href)
  const server = await createServer({ root, server: { host: '127.0.0.1', port: 0 }, plugins: [{
    name: 'backup-scope-fixture',
    configResolved(config) { config.server.proxy = {} },
    resolveId(id) { return id === '/backup-qa.js' ? '\0backup-qa' : undefined },
    load(id) { return id === '\0backup-qa' ? entry : undefined },
  }] })
  await server.listen()
  const base = `http://127.0.0.1:${server.httpServer.address().port}`
  let browser
  try {
    browser = await chromium.launch({ headless: true })
    await mkdir(process.env.QA_OUTPUT_DIR, { recursive: true })
    for (const width of [1440, 390, 320]) {
      const page = await browser.newPage({ viewport: { width, height: 900 } })
      const errors = []
      page.on('pageerror', error => errors.push(error.message))
      await page.route('**/*', (route) => {
        const url = new URL(route.request().url())
        assert.equal(url.origin, base)
        if (url.pathname === '/qa')
          return route.fulfill({ contentType: 'text/html', body: '<html><meta name="viewport" content="width=device-width,initial-scale=1"><div id="app"></div><script type="module" src="/backup-qa.js"></script></html>' })
        return route.continue()
      })
      await page.goto(`${base}/qa`)
      await page.getByText('备份记录', { exact: true }).waitFor()
      await page.getByText(/业务备份保留账号/).waitFor()
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth), false)
      assert.deepEqual(errors, [])
      await page.screenshot({ path: join(process.env.QA_OUTPUT_DIR, `backup-${width}.png`), fullPage: true })
      await page.close()
    }
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
