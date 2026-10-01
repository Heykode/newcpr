import type { Ref } from 'vue'
import type { Account } from '@/api'
import type { ResetBatch, ResetInventory } from '@/api/modules/reset-credits'
import { computed, onScopeDispose, shallowRef, watch } from 'vue'
import { confirmResetBatch, getResetBatches, getResetInventory, previewResetBatch, refreshResetInventory, retryResetBatch } from '@/api/modules/reset-credits'
import { errorMessage } from '@/utils/async'

export function useBatchResetCredits(options: {
  accounts: Ref<Account[]>
  selectedIds: Ref<Set<string>>
  reload: () => Promise<unknown>
}) {
  const inventory = shallowRef<Record<string, ResetInventory>>({})
  const busy = shallowRef(false)
  const open = shallowRef(false)
  const error = shallowRef('')
  const preview = shallowRef<ResetBatch | null>(null)
  const viewingPreview = shallowRef(false)
  const batches = shallowRef<ResetBatch[]>([])
  const selectedBatchId = shallowRef('')
  const resetType = shallowRef('')
  const frozenIds = shallowRef<string[]>([])
  let disposed = false
  let polling = false
  let generation = 0
  const controller = new AbortController()
  const completed = new Set<string>()
  const current = computed(() => viewingPreview.value
    ? preview.value
    : batches.value.find(b => b.id === selectedBatchId.value) ?? batches.value[0] ?? null)
  const activeCount = computed(() => batches.value.filter(b => b.items.some(i => i.status === 'queued' || i.status === 'running')).length)
  const typeOptions = computed(() => {
    const types = new Set<string>()
    for (const id of frozenIds.value) {
      for (const credit of inventory.value[id]?.credits?.credits ?? []) {
        if (credit.resetType)
          types.add(credit.resetType)
      }
    }
    return [{ label: '自动选择（同类型内到期优先）', value: '' }, ...[...types].sort().map(value => ({ label: value, value }))]
  })
  function apply(rows: ResetInventory[]) {
    if (disposed)
      return
    const next = { ...inventory.value }
    for (const row of rows) {
      if (!next[row.accountId] || row.checkedAt >= next[row.accountId]!.checkedAt)
        next[row.accountId] = row
    }
    inventory.value = next
  }
  async function loadCache() {
    const ids = options.accounts.value.map(a => a.id)
    if (!ids.length)
      return
    apply(await getResetInventory(ids, { silent: true, signal: controller.signal }))
  }
  async function poll() {
    if (disposed || polling)
      return
    polling = true
    try {
      const result = await getResetBatches({ silent: true, signal: controller.signal })
      if (disposed)
        return
      batches.value = result
      if (!result.some(batch => batch.id === selectedBatchId.value))
        selectedBatchId.value = result[0]?.id ?? ''
      let changed = false
      for (const batch of result) {
        for (const item of batch.items) {
          if (item.status === 'succeeded' && !completed.has(item.redeemRequestId)) {
            completed.add(item.redeemRequestId)
            changed = true
          }
        }
      }
      if (changed) {
        await options.reload()
        await loadCache()
      }
    }
    catch (e) {
      if (!disposed && open.value)
        error.value = errorMessage(e, '重置任务读取失败')
    }
    finally { polling = false }
  }
  async function refreshSelected() {
    if (busy.value || !options.selectedIds.value.size)
      return
    const ids = [...options.selectedIds.value]
    busy.value = true
    error.value = ''
    try {
      apply(await refreshResetInventory(ids, { silent: true, signal: controller.signal }))
    }
    catch (e) {
      if (!disposed) {
        error.value = errorMessage(e)
        viewingPreview.value = false
        preview.value = null
        open.value = true
      }
    }
    finally { busy.value = false }
  }
  async function prepare(reuseSelection = false) {
    if (busy.value)
      return
    if (!reuseSelection) {
      frozenIds.value = [...options.selectedIds.value]
      resetType.value = ''
    }
    if (!frozenIds.value.length)
      return
    const version = ++generation
    busy.value = true
    error.value = ''
    preview.value = null
    viewingPreview.value = true
    open.value = true
    try {
      const batch = await previewResetBatch(frozenIds.value, resetType.value || undefined, { silent: true, signal: controller.signal })
      if (!disposed && version === generation) {
        preview.value = batch
        apply(await getResetInventory(frozenIds.value, { silent: true, signal: controller.signal }))
      }
    }
    catch (e) {
      if (!disposed && version === generation)
        error.value = errorMessage(e)
    }
    finally { busy.value = false }
  }
  async function confirm() {
    const batch = preview.value
    if (busy.value || !batch || batch.confirmed)
      return
    busy.value = true
    error.value = ''
    try {
      const result = await confirmResetBatch(batch.id)
      if (disposed)
        return
      selectedBatchId.value = result.id
      preview.value = null
      viewingPreview.value = false
      batches.value = [result, ...batches.value.filter(b => b.id !== result.id)]
      await poll()
    }
    catch (e) {
      // Retain the same preview ID: a lost confirmation response must not create a new job.
      if (!disposed)
        error.value = errorMessage(e, '确认结果未知，请继续确认本次任务')
    }
    finally { busy.value = false }
  }
  async function retry(accountId: string) {
    if (busy.value || !current.value)
      return
    busy.value = true
    error.value = ''
    try {
      await retryResetBatch(current.value.id, accountId)
      await poll()
    }
    catch (e) {
      if (!disposed)
        error.value = errorMessage(e)
    }
    finally { busy.value = false }
  }
  function history() {
    if (busy.value)
      return
    preview.value = null
    viewingPreview.value = false
    error.value = ''
    open.value = true
    void poll()
  }
  watch(() => options.accounts.value, () => {
    void loadCache().catch(() => {})
  }, { immediate: true })
  void poll()
  const timer = setInterval(() => {
    if (open.value || activeCount.value)
      void poll()
  }, 3000)
  onScopeDispose(() => {
    disposed = true
    generation++
    clearInterval(timer)
    controller.abort()
  })
  return { inventory, busy, open, error, preview, batches, current, activeCount, selectedBatchId, resetType, typeOptions, refreshSelected, prepare, confirm, retry, history }
}
