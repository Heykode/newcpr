// RFC 6238, six digits and a 30-second period. Secret stays in modal memory.
export async function currentTotp(secret: string, timestamp = Date.now()): Promise<string> {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'
  let buffer = 0
  let bits = 0
  const bytes: number[] = []
  for (const char of secret.toUpperCase().replace(/[\s=-]/g, '')) {
    const value = alphabet.indexOf(char)
    if (value < 0)
      throw new Error('2FA 密钥格式无效')
    buffer = (buffer << 5) | value
    bits += 5
    if (bits >= 8) {
      bits -= 8
      bytes.push((buffer >>> bits) & 255)
    }
  }
  if (!bytes.length)
    throw new Error('缺少 2FA 密钥')
  const key = await crypto.subtle.importKey('raw', new Uint8Array(bytes), { name: 'HMAC', hash: 'SHA-1' }, false, ['sign'])
  const counter = new ArrayBuffer(8)
  new DataView(counter).setBigUint64(0, BigInt(Math.floor(timestamp / 30000)))
  const digest = new Uint8Array(await crypto.subtle.sign('HMAC', key, counter))
  const offset = digest[digest.length - 1]! & 15
  const number = new DataView(digest.buffer).getUint32(offset) & 0x7FFFFFFF
  return String(number % 1000000).padStart(6, '0')
}
