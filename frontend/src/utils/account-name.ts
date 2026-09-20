export function normalizeAccountName(value: string | null | undefined): string | null {
  if (value && [...value].some(char => /\p{Cc}/u.test(char)))
    throw new Error('账号名称不能包含控制字符')
  const name = value?.trim() ?? ''
  if ([...name].length > 128)
    throw new Error('账号名称不能超过 128 个字符')
  return name || null
}
