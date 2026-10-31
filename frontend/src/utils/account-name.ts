export function normalizeAccountName(value: string | null | undefined): string | null {
  if (value && [...value].some(char => /\p{Cc}/u.test(char)))
    throw new Error('账号名称不能包含控制字符')
  if (new TextEncoder().encode(value ?? '').length > 512)
    throw new Error('账号名称不能超过 512 个 UTF-8 字节')
  const name = value?.trim() ?? ''
  return name || null
}
