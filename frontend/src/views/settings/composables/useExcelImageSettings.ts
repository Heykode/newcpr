import type { Ref } from 'vue'
import type { RequestTuning } from '@/api/modules/settings'
import { computed, reactive, ref } from 'vue'

const MiB = 1024 * 1024

export const excelImageFields = [
  { key: 'excelImageMaxBytes', section: 'limits', label: '单张图片上限', unit: 'MiB', scale: MiB, min: 1, max: 128 * MiB },
  { key: 'excelImageTotalBytes', section: 'limits', label: '每请求图片总大小', unit: 'MiB', scale: MiB, min: 1, max: 128 * MiB },
  { key: 'excelImageMaxCount', section: 'limits', label: '每请求图片数上限', unit: '张', scale: 1, min: 1, max: 4096 },
  { key: 'excelImageWarningRemaining', section: 'warn', label: '剩余多少张时预警', unit: '张', scale: 1, min: 1, max: 4096 },
  { key: 'excelImageCompactReserve', section: 'warn', label: '压缩预留图片数', unit: '张', scale: 1, min: 1, max: 4096 },
  { key: 'excelImageRelayBytes', section: 'relay', label: '进程暂存容量', unit: 'MiB', scale: MiB, min: MiB, max: 16384 * MiB },
  { key: 'excelImageRelayEntries', section: 'relay', label: '进程暂存图片数', unit: '张', scale: 1, min: 1, max: 65536 },
  { key: 'excelImageRelayRequests', section: 'relay', label: '最大在途中转请求数', unit: '个', scale: 1, min: 1, max: 512 },
  { key: 'excelImageRelayDownloads', section: 'relay', label: '中转下载并发上限', unit: '个', scale: 1, min: 1, max: 128 },
  { key: 'excelImageRelayTtlMinutes', section: 'relay', label: '链接有效期', unit: '分钟', scale: 1, min: 1, max: 1440 },
] as const

type ImageField = typeof excelImageFields[number]
type ImageFieldKey = ImageField['key']

function validationError(field: ImageField, raw: string): string {
  if (!raw.trim())
    return `请填写${field.label}`
  const value = Number(raw) * field.scale
  if (!Number.isFinite(value))
    return '请输入有效数字'
  if (!Number.isSafeInteger(value))
    return field.scale === MiB ? '换算后须为整数字节，请调整小数精度' : '请输入整数'
  if (value < field.min || value > field.max) {
    if (field.scale === MiB && field.min === 1)
      return `范围：1 字节至 ${field.max / MiB} MiB`
    return `范围：${field.min / field.scale}–${field.max / field.scale} ${field.unit}`
  }
  return ''
}

export function useExcelImageSettings(tuning: Ref<RequestTuning>) {
  const drafts = reactive<Partial<Record<ImageFieldKey, string>>>({})
  const lastRelayUrl = ref('')
  const fields = excelImageFields.map((field) => {
    const input = computed({
      // Do not round: even legacy values that are not whole MiB must roundtrip.
      get: () => drafts[field.key] ?? String(tuning.value[field.key] / field.scale),
      set: (raw: string) => {
        drafts[field.key] = raw
        if (!validationError(field, raw))
          tuning.value[field.key] = Number(raw) * field.scale
      },
    })
    return { ...field, input, error: computed(() => validationError(field, input.value)) }
  })
  const errors = computed(() => fields.filter(field => field.error.value).map(field => `${field.label}：${field.error.value}`))
  const mode = computed({
    get: () => tuning.value.excelImageTransport?.mode ?? 'inherit',
    set: (value: string) => {
      if (!['inherit', 'native', 'relay'].includes(value))
        return
      const previous = tuning.value.excelImageTransport
      if (previous?.mode === 'relay')
        lastRelayUrl.value = previous.publicUrl
      tuning.value.excelImageTransport = value === 'relay'
        ? { mode: 'relay', publicUrl: lastRelayUrl.value }
        : value === 'native' ? { mode: 'native' } : null
    },
  })
  const relayUrl = computed({
    get: () => tuning.value.excelImageTransport?.mode === 'relay' ? tuning.value.excelImageTransport.publicUrl : '',
    set: (publicUrl: string) => {
      lastRelayUrl.value = publicUrl
      tuning.value.excelImageTransport = { mode: 'relay', publicUrl }
    },
  })

  function resetDrafts() {
    for (const field of excelImageFields)
      delete drafts[field.key]
    lastRelayUrl.value = tuning.value.excelImageTransport?.mode === 'relay' ? tuning.value.excelImageTransport.publicUrl : ''
  }

  return { fields, errors, mode, relayUrl, resetDrafts }
}

export type ExcelImageSettings = ReturnType<typeof useExcelImageSettings>
