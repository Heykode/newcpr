import { isRecord } from '@/utils/object'

export interface ImportDocument {
  provider: 'openai' | 'xai'
  document: Record<string, unknown>
}

export function splitAccountDocuments(entry: ImportDocument): ImportDocument[] {
  // xAI owns refresh-token deduplication across the complete document.
  if (entry.provider !== 'openai')
    return [entry]
  const document = entry.document
  const data = isRecord(document.data) && Array.isArray(document.data.accounts) ? document.data : document
  if (!Array.isArray(data.accounts) || !data.accounts.length)
    return [entry]
  return data.accounts.map((account) => {
    const item: Record<string, unknown> = { ...data, accounts: [account] }
    if (Array.isArray(data.proxies)) {
      const key = isRecord(account) ? account.proxy_key : undefined
      item.proxies = data.proxies.filter(proxy => typeof key === 'string' && isRecord(proxy) && proxy.proxy_key === key)
    }
    return { provider: entry.provider, document: data === document ? item : { ...document, data: item } }
  })
}

export function accountImportBatches<T>(items: T[]): T[][] {
  const encoder = new TextEncoder()
  const maximumBytes = 4 * 1024 * 1024
  const envelopeBytes = encoder.encode(JSON.stringify({ submissionId: '00000000-0000-0000-0000-000000000000', items: [] })).length
  const batches: T[][] = []
  let batch: T[] = []
  let bytes = envelopeBytes
  for (const item of items) {
    const itemBytes = encoder.encode(JSON.stringify(item)).length
    if (envelopeBytes + itemBytes > maximumBytes)
      throw new Error('单条导入文档超过 4 MiB，请拆分文件后重试')
    if (batch.length >= 200 || bytes + itemBytes + Number(batch.length > 0) > maximumBytes) {
      batches.push(batch)
      batch = []
      bytes = envelopeBytes
    }
    bytes += itemBytes + Number(batch.length > 0)
    batch.push(item)
  }
  if (batch.length)
    batches.push(batch)
  return batches
}
