// Local-only synthetic preview. Never forwards a request or contacts a model.
import { Buffer } from 'node:buffer'
import { randomUUID } from 'node:crypto'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { createServer } from 'vite'

const config = {
  accountId: 'preview-account',
  model: 'example-model',
  enabled: false,
  cron: '0 */6 * * *',
  timezone: 'Asia/Shanghai',
  repetitions: 1,
  prompt: '这是界面预览用题目。',
  referenceAnswer: '示例参考答案',
  reasoningEffort: null,
  judgeGroupId: 'preview-group',
  judgeModel: 'example-judge',
  judgePrompt: '比较实际答案与参考答案。',
}
let rules = [{
  id: 'preview-rule',
  revision: 1,
  config,
  nextRunAt: new Date().toISOString(),
  running: false,
  pending: false,
  lastStatus: null,
  lastRunAt: null,
}]
async function main() {
  const server = await createServer({
    root: fileURLToPath(new URL('..', import.meta.url)),
    server: { host: '127.0.0.1', port: Number(process.env.QA_PORT || 5199), strictPort: true },
    plugins: [{
      name: 'quality-preview-no-upstream',
      configResolved(resolved) {
        resolved.server.proxy = {}
      },
      configureServer(vite) {
        vite.middlewares.use(async (request, response, next) => {
          const url = new URL(request.url ?? '/', 'http://127.0.0.1')
          if (!url.pathname.startsWith('/dev/'))
            return next()
          const path = url.pathname.replace('/dev', '')
          let data
          if (request.method === 'GET') {
            if (path === '/api/admin/auth/status')
              data = { authenticated: true }
            if (path === '/api/admin/system/version')
              data = { version: 'quality-ui-preview', buildType: 'test' }
            if (path === '/api/admin/accounts')
              data = { items: [{ id: 'preview-account', name: '示例账号（非真实数据）' }], page: { page: 1, pageSize: 50, total: 1, totalPages: 1 } }
            if (path === '/api/admin/account-groups')
              data = { items: [{ id: 'preview-group', name: '示例判题分组', enabled: true }], page: { page: 1, pageSize: 50, total: 1, totalPages: 1 } }
            if (path === '/api/admin/quality-ops/rules')
              data = rules
            if (path === '/api/admin/quality-ops/runs')
              data = []
          }
          else if (request.method === 'POST' && path.startsWith('/api/admin/quality-ops/')) {
            try {
              const chunks = []
              let size = 0
              for await (const chunk of request) {
                size += chunk.length
                if (size > 128_000)
                  throw new Error('preview payload too large')
                chunks.push(chunk)
              }
              const body = JSON.parse(Buffer.concat(chunks).toString())
              if (path.endsWith('/save')) {
                const rule = { id: body.id || randomUUID(), revision: (body.revision || 0) + 1, config: body.config, nextRunAt: new Date().toISOString(), running: false, pending: false, lastStatus: null, lastRunAt: null }
                rules = [...rules.filter(item => item.id !== rule.id), rule]
                data = rule
              }
              if (path.endsWith('/delete')) {
                rules = rules.filter(rule => rule.id !== body.id)
                data = null
              }
            }
            catch {}
          }
          response.statusCode = data === undefined ? 405 : 200
          response.setHeader('Content-Type', 'application/json')
          response.setHeader('Cache-Control', 'no-store')
          response.end(JSON.stringify({ code: response.statusCode, message: data === undefined ? '仅界面预览，不执行真实检测' : 'ok', data: data ?? null }))
        })
      },
    }],
  })
  await server.listen()
  process.stdout.write(`Synthetic preview only: http://127.0.0.1:${server.config.server.port}/quality-ops\n`)
}
main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
