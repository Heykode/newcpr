/* eslint-disable test/no-import-node-test -- these regressions use Node's built-in runner. */
import assert from 'node:assert/strict'
import { Buffer } from 'node:buffer'
import { readFileSync } from 'node:fs'
import { createServer } from 'node:http'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'

const require = createRequire(import.meta.url)
const filename = new URL('../src/api/modules/account-connection-test.ts', import.meta.url)
const { outputText } = ts.transpileModule(readFileSync(filename, 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
})
const payload = { accountId: 'account-a', modelId: 'test-model', interface: 'responses', stream: true, prompt: 'OK' }
const complete = 'data: {"type":"test_complete","success":true}\n\n'

function loadStream(fetchImpl = fetch, base = '') {
  const exports = {}
  runInNewContext(outputText, {
    exports,
    require: name => name === '../constants' ? { API_BASE_URL: base } : require(name),
    fetch: fetchImpl,
    TextDecoder,
  }, { filename: String(filename) })
  return exports.streamAccountConnectionTest
}

test('a maximum UTF-8 prompt passes in POST body behind an 8KB HTTP header limit; the legacy URL fails', async () => {
  const requests = []
  const server = createServer({ maxHeaderSize: 8192 }, async (req, res) => {
    const chunks = []
    for await (const chunk of req)
      chunks.push(chunk)
    requests.push({ method: req.method, url: req.url, body: JSON.parse(Buffer.concat(chunks).toString()) })
    res.writeHead(200, { 'Content-Type': 'text/event-stream' })
    res.end(complete)
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  const base = `http://127.0.0.1:${server.address().port}`
  const prompt = `${'中'.repeat(1365)}a`
  const longPayload = { ...payload, prompt }
  try {
    const encoded = new URLSearchParams(longPayload)
    const legacy = await fetch(`${base}/api/admin/accounts/connection-test?${encoded}`)
    assert.equal(legacy.status, 431)
    await legacy.text()
    const events = []
    await loadStream(fetch, base)(longPayload, new AbortController().signal, data => events.push(JSON.parse(data)))
    assert.equal(Buffer.byteLength(prompt), 4096)
    assert.equal(requests.length, 1)
    assert.deepEqual(requests[0], { method: 'POST', url: '/api/admin/accounts/connection-test', body: longPayload })
    assert.equal(events[0].type, 'test_complete')
  }
  finally {
    server.closeAllConnections()
    await new Promise(resolve => server.close(resolve))
  }
})

test('SSE parser preserves fragmented UTF-8, CRLF, comments, multiline data and every burst event', async () => {
  const wire = `: keepalive\r\n\r\ndata: {"type":"content",\r\ndata: "text":"你好"}\r\n\r\n${
    Array.from({ length: 50 }, (_, index) => `data: {"type":"content","text":"${index}"}\n\n`).join('')
  }${complete}`
  const bytes = new TextEncoder().encode(wire)
  let index = 0
  const body = new ReadableStream({
    pull(controller) {
      if (index === bytes.length)
        controller.close()
      else
        controller.enqueue(bytes.slice(index, ++index))
    },
  })
  let calls = 0
  const events = []
  await loadStream(async (_url, options) => {
    calls += 1
    assert.equal(options.method, 'POST')
    assert.equal(options.credentials, 'include')
    assert.equal(options.headers.Accept, 'text/event-stream')
    return new Response(body, { headers: { 'Content-Type': 'text/event-stream; charset=utf-8' } })
  })(payload, new AbortController().signal, data => events.push(JSON.parse(data)))
  assert.equal(calls, 1)
  assert.equal(events.length, 52)
  assert.equal(events[0].text, '你好')
  assert.equal(events[50].text, '49')
  assert.equal(events[51].type, 'test_complete')
})

test('HTTP errors and login HTML are visible failures without automatic retries', async () => {
  for (const [response, expected] of [
    [new Response('{"code":400,"message":"提示词无效"}', { status: 400 }), /提示词无效/],
    [new Response('<h1>Too large</h1>', { status: 413 }), /413/],
    [new Response('<html>Login</html>', { headers: { 'Content-Type': 'text/html' } }), /未返回事件流/],
  ]) {
    let calls = 0
    await assert.rejects(loadStream(async () => {
      calls += 1
      return response
    })(payload, new AbortController().signal, assert.fail), expected)
    assert.equal(calls, 1)
  }
})

test('terminal cancellation discards trailing events and releases the response stream', async () => {
  const controller = new AbortController()
  let cancelled = false
  const body = new ReadableStream({
    start(stream) {
      stream.enqueue(new TextEncoder().encode(`${complete}data: {"type":"content","text":"stale"}\n\n`))
    },
    cancel() { cancelled = true },
  })
  const events = []
  await loadStream(async () => new Response(body, { headers: { 'Content-Type': 'text/event-stream' } }))(
    payload,
    controller.signal,
    (data) => {
      events.push(JSON.parse(data))
      controller.abort()
    },
  )
  assert.equal(events.length, 1)
  assert.equal(cancelled, true)
  assert.equal(body.locked, false)
})

test('oversized or malformed event failures cancel the stream instead of hanging', async () => {
  for (const wire of ['data: {bad}\n\n', `data: ${'a'.repeat(1024 * 1024 + 1)}`]) {
    let cancelled = false
    const body = new ReadableStream({
      start(stream) { stream.enqueue(new TextEncoder().encode(wire)) },
      cancel() { cancelled = true },
    })
    await assert.rejects(loadStream(async () => new Response(body, { headers: { 'Content-Type': 'text/event-stream' } }))(
      payload,
      new AbortController().signal,
      data => JSON.parse(data),
    ))
    assert.equal(cancelled, true)
  }
})
