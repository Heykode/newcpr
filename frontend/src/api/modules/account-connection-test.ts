import { createParser } from 'eventsource-parser'
import { API_BASE_URL } from '../constants'

export interface AccountConnectionTestRequest {
  accountId: string
  modelId: string
  interface: 'responses'
  stream: boolean
  prompt: string
}

export async function streamAccountConnectionTest(
  payload: AccountConnectionTestRequest,
  signal: AbortSignal,
  onMessage: (data: string) => void,
) {
  const response = await fetch(`${API_BASE_URL}/api/admin/accounts/connection-test`, {
    method: 'POST',
    credentials: 'include',
    headers: { 'Content-Type': 'application/json', 'Accept': 'text/event-stream' },
    body: JSON.stringify(payload),
    signal,
  })
  if (!response.ok) {
    const data: unknown = await response.json().catch(() => null)
    const message = data && typeof data === 'object' && 'message' in data && typeof data.message === 'string'
      ? data.message
      : `测试请求失败（HTTP ${response.status}）`
    throw new Error(message)
  }
  if (response.headers.get('content-type')?.split(';')[0]?.trim().toLowerCase() !== 'text/event-stream') {
    await response.body?.cancel()
    throw new Error('测试接口未返回事件流，请检查登录状态或代理配置')
  }
  if (!response.body)
    throw new Error('测试接口未返回响应内容')

  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  const parser = createParser({
    maxBufferSize: 1024 * 1024,
    onEvent: (event) => {
      if (!signal.aborted)
        onMessage(event.data)
    },
    onError: (error) => {
      if (error.type === 'max-buffer-size-exceeded')
        throw new Error('测试响应单条事件过大')
    },
  })
  try {
    while (!signal.aborted) {
      const { done, value } = await reader.read()
      if (done) {
        parser.feed(decoder.decode())
        break
      }
      parser.feed(decoder.decode(value, { stream: true }))
    }
  }
  finally {
    await reader.cancel().catch(() => {})
    reader.releaseLock()
  }
}
