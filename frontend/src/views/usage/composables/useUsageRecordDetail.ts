import type { UsageDisplayRecord, UsageViewModel } from '../utils/records'
import { computed, shallowRef, watch } from 'vue'
import { getUsageRecordDetail } from '@/api'
import { useRequestState } from '@/composables/useRequestState'
import { normalizeUsageRecord } from '../utils/records'

export function useUsageRecordDetail() {
  const showDetailModal = shallowRef(false)
  const selectedUsageRecord = shallowRef<UsageViewModel | null>(null)
  const selectedRecord = shallowRef<UsageDisplayRecord | null>(null)
  const request = useRequestState()
  const detailIsError = computed(() => ['failed', 'incomplete'].includes(selectedUsageRecord.value?.logicalOutcome ?? selectedRecord.value?.outcome ?? ''))

  async function loadDetail() {
    if (!selectedRecord.value || !showDetailModal.value)
      return
    const sequence = request.start()
    try {
      const detail = await getUsageRecordDetail({ id: selectedRecord.value.id }, { signal: request.signal, silent: true })
      if (request.isCurrent(sequence))
        selectedUsageRecord.value = normalizeUsageRecord(detail)
    }
    catch (cause) {
      request.fail(sequence, cause)
    }
    finally {
      request.finish(sequence)
    }
  }

  function handleViewDetail(record: UsageDisplayRecord) {
    selectedRecord.value = record
    selectedUsageRecord.value = null
    showDetailModal.value = true
    void loadDetail()
  }

  watch(showDetailModal, (open) => {
    if (!open)
      request.invalidate({ resetLoading: false })
  }, { flush: 'sync' })

  return {
    showDetailModal,
    selectedUsageRecord,
    handleViewDetail,
    detailIsError,
    detailLoading: request.loading,
    detailError: request.error,
    reloadDetail: loadDetail,
  }
}
