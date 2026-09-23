import type { BarkLevel } from '@/api'

export const barkLevelOptions: { label: string, value: BarkLevel }[] = [
  { label: '普通提醒', value: 'active' },
  { label: '时效性提醒', value: 'timeSensitive' },
  { label: '重要警告（静音仍响铃）', value: 'critical' },
  { label: '静默记录', value: 'passive' },
]

export const barkLevelHints: Record<BarkLevel, string> = {
  active: '跟随手机静音设置；静音、勿扰时也要响铃，请选「重要警告」。',
  timeSensitive: '可在专注模式下显示，响铃仍受手机静音设置影响。',
  critical: '静音、勿扰时仍可响铃；需在 iPhone 为 Bark 开启「重要警告」，且音量大于 0。',
  passive: '仅进入通知列表，不亮屏、不响铃。',
}
