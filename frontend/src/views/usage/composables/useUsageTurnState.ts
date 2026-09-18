import { shallowRef, watch } from 'vue'
import { getUsageRecordDetail } from '@/api'
import { isRecord } from '@/utils/object'

export function useUsageTurnState(requestId: () => string, visible: () => boolean) {
  const value = shallowRef<string | null>(null)
  const loading = shallowRef(false)
  const failed = shallowRef(false)
  const revision = shallowRef(0)

  watch([requestId, visible, revision], async ([id, open], _previous, onCleanup) => {
    value.value = null
    failed.value = false
    loading.value = false
    if (!open || !id)
      return
    let active = true
    const controller = new AbortController()
    onCleanup(() => {
      active = false
      controller.abort()
      value.value = null
    })
    loading.value = true
    try {
      const detail = await getUsageRecordDetail({ id }, { signal: controller.signal })
      if (!active)
        return
      const state = detail.requestId === id && isRecord(detail.metadata.turnState)
        ? detail.metadata.turnState.injectedState
        : null
      value.value = typeof state === 'string' && state.length > 0 && state.length <= 2048 ? state : null
    }
    catch {
      if (active)
        failed.value = true
    }
    finally {
      if (active)
        loading.value = false
    }
  }, { immediate: true })

  return { value, loading, failed, retry: () => revision.value++ }
}
