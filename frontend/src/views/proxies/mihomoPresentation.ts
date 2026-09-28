import type { CountryFilter, MihomoNode, NodeCheck } from '@/api/modules/mihomo'

const chineseRegions = new Intl.DisplayNames(['zh-CN'], { type: 'region' })
const englishRegions = new Intl.DisplayNames(['en'], { type: 'region' })

export function regionLabel(code: string): string {
  if (!/^[A-Z]{2}$/.test(code))
    return code
  return `${chineseRegions.of(code) ?? code} ${code}`
}

export function regionMatches(code: string, search: string): boolean {
  const english = /^[A-Z]{2}$/.test(code) ? englishRegions.of(code) ?? '' : ''
  return `${regionLabel(code)} ${english}`.toLowerCase().includes(search.trim().toLowerCase())
}

export function sameCountryFilter(left: CountryFilter, right: CountryFilter): boolean {
  return left.mode === right.mode
    && left.allowUnknown === right.allowUnknown
    && left.dynamicProviderManaged === right.dynamicProviderManaged
    && [...left.codes].sort().join(',') === [...right.codes].sort().join(',')
}

export function countryBlockReason(node: MihomoNode, filter?: CountryFilter): string | null {
  if (!node.countryBlocked)
    return null
  if (node.dynamic)
    return '动态地区未准入'
  if (!node.countryCode) {
    if (node.countryError)
      return '地区检测失败，暂不准入'
    return node.countryCheckedAt ? '地区未知，暂不准入' : '地区未检测，暂不准入'
  }
  if (filter?.mode === 'exclude' && filter.codes.includes(node.countryCode))
    return `已排除地区：${regionLabel(node.countryCode)}`
  if (filter?.mode === 'include' && !filter.codes.includes(node.countryCode))
    return `不在允许地区内：${regionLabel(node.countryCode)}`
  return '地区规则限制，暂不准入'
}

export function qualityStatus(quality: NodeCheck['quality'] | undefined): string {
  if (!quality)
    return '未完成检测'
  if (quality.checks.some(item => item.status === 'challenge'))
    return '遇到挑战'
  if (quality.checks.some(item => item.status === 'fail'))
    return '存在失败'
  if (quality.checks.some(item => item.status === 'warn'))
    return '存在告警'
  return quality.checks.length && quality.checks.every(item => item.status === 'pass') ? '连通检测通过' : '未完成检测'
}
