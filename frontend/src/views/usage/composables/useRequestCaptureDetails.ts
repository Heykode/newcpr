import type { CapturePage, CaptureRecord } from '@/api/modules/request-capture'
import { computed, shallowRef, watch } from 'vue'
import { getCapturesForRequest, readRequestCapture } from '@/api/modules/request-capture'

/** Request-owned payloads are never cached in the list, preferences or safe export. */
export function useRequestCaptureDetails(requestId: () => string) {
  const records = shallowRef<CaptureRecord[]>([])
  const selectedId = shallowRef('')
  const offset = shallowRef(0)
  const page = shallowRef<CapturePage | null>(null)
  const loading = shallowRef(false)
  const reading = shallowRef(false)
  const error = shallowRef('')
  const readError = shallowRef('')
  const revision = shallowRef(0)
  const bodyRevision = shallowRef(0)
  const selected = computed(() => records.value.find(record => record.id === selectedId.value && record.requestId === requestId()))

  watch([requestId, revision], async ([id], _previous, onCleanup) => {
    let active = true
    const controller = new AbortController()
    onCleanup(() => {
      active = false
      controller.abort()
      records.value = []
      selectedId.value = ''
      page.value = null
    })
    records.value = []
    selectedId.value = ''
    page.value = null
    error.value = ''
    loading.value = true
    try {
      const result = await getCapturesForRequest(id, { signal: controller.signal, silent: true })
      if (!active)
        return
      if (result.some(record => record.requestId !== id))
        throw new Error('capture_request_mismatch')
      records.value = result
      selectedId.value = result[0]?.id ?? ''
    }
    catch {
      if (active)
        error.value = '读取关联采集记录失败'
    }
    finally {
      if (active)
        loading.value = false
    }
  }, { immediate: true })

  watch(selectedId, () => offset.value = 0, { flush: 'sync' })
  watch([selected, offset, bodyRevision], async ([record, position], _previous, onCleanup) => {
    let active = true
    const controller = new AbortController()
    onCleanup(() => {
      active = false
      controller.abort()
      page.value = null
    })
    page.value = null
    readError.value = ''
    reading.value = false
    if (!record)
      return
    reading.value = true
    try {
      const result = await readRequestCapture(record.id, position, { signal: controller.signal, silent: true })
      if (active)
        page.value = result
    }
    catch {
      if (active)
        readError.value = '读取采集材料失败，记录可能已过期或被删除'
    }
    finally {
      if (active)
        reading.value = false
    }
  }, { immediate: true })

  return {
    records,
    selectedId,
    selected,
    offset,
    page,
    loading,
    reading,
    error,
    readError,
    refresh: () => revision.value++,
    retry: () => bodyRevision.value++,
  }
}
