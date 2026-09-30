import type { CleanupConfig, CleanupFootprint, CleanupPreview, CleanupState } from '@/api/modules/log-cleanup'
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { cancelCleanup, getCleanupState, getCleanupUsage, previewCleanup, saveCleanupConfig, startCleanup } from '@/api/modules/log-cleanup'
import { toast } from '@/components/base/BaseToast'
import { errorMessage } from '@/utils/async'
import { cleanupValidation } from '../components/cleanup/cleanup'

export function useLogCleanup() {
  const state = ref<CleanupState | null>(null)
  const draft = ref<CleanupConfig | null>(null)
  const saved = ref('')
  const draftRevision = ref(0)
  const usage = ref<CleanupFootprint | null>(null)
  const preview = ref<CleanupPreview | null>(null)
  const confirming = ref(false)
  const busy = ref(false)
  const refreshing = ref(false)
  const loading = ref(true)
  const error = ref('')
  const usageError = ref('')
  const controller = new AbortController()
  let disposed = false
  let timer: ReturnType<typeof setTimeout> | undefined
  const dirty = computed(() => draft.value !== null && JSON.stringify(draft.value) !== saved.value)
  const running = computed(() => state.value?.job?.status === 'running')
  const selected = computed(() => draft.value !== null && !cleanupValidation(draft.value))

  function apply(next: CleanupState, replace = false) {
    if (!draft.value || replace || !dirty.value) {
      draft.value = structuredClone(next.config)
      saved.value = JSON.stringify(next.config)
      draftRevision.value = next.revision
    }
    state.value = next
  }
  async function refreshUsage() {
    if (refreshing.value)
      return
    refreshing.value = true
    usageError.value = ''
    try {
      const next = await getCleanupUsage({ signal: controller.signal })
      if (!disposed)
        usage.value = next
    }
    catch (cause) {
      if (!disposed) {
        usage.value = null
        usageError.value = errorMessage(cause)
      }
    }
    finally {
      refreshing.value = false
    }
  }
  async function poll() {
    try {
      const previous = state.value?.job
      const next = await getCleanupState({ signal: controller.signal })
      if (disposed)
        return
      apply(next)
      error.value = ''
      if (previous?.status === 'running' && (next.job?.status !== 'running' || next.job.id !== previous.id)) {
        if (next.job?.id === previous.id) {
          if (next.job.status === 'failed')
            toast.error('部分项目清理失败，请检查后重试')
          else if (next.job.status === 'cancelled')
            toast.success('清理已停止，已完成的删除不会撤销')
          else
            toast.success('清理完成')
        }
        await refreshUsage()
      }
    }
    catch (cause) {
      if (!disposed)
        error.value = errorMessage(cause)
    }
    finally {
      if (!disposed)
        timer = setTimeout(poll, running.value ? 2000 : 10000)
    }
  }
  async function action(fn: () => Promise<void>) {
    if (busy.value)
      return
    busy.value = true
    error.value = ''
    try {
      await fn()
    }
    catch (cause) { error.value = errorMessage(cause) }
    finally { busy.value = false }
  }
  async function save() {
    if (!draft.value)
      return
    const config = JSON.parse(JSON.stringify(draft.value)) as CleanupConfig
    await action(async () => {
      await saveCleanupConfig(draftRevision.value, config)
      apply(await getCleanupState({ signal: controller.signal }), true)
      toast.success('清理设置已保存')
    })
  }
  async function requestCleanup() {
    if (dirty.value || !selected.value || running.value)
      return
    await action(async () => {
      preview.value = await previewCleanup()
      confirming.value = true
    })
  }
  async function confirmCleanup() {
    if (!preview.value)
      return
    const confirmed = preview.value
    await action(async () => {
      const job = await startCleanup(confirmed)
      if (state.value)
        state.value.job = job
      confirming.value = false
      toast.success('清理任务已开始')
    })
  }
  async function stop() {
    const id = state.value?.job?.id
    if (id) {
      await action(async () => {
        await cancelCleanup(id)
        apply(await getCleanupState({ signal: controller.signal }))
        await refreshUsage()
        toast.success('清理已停止')
      })
    }
  }
  onMounted(async () => {
    await Promise.all([poll(), refreshUsage()])
    loading.value = false
  })
  onBeforeUnmount(() => {
    disposed = true
    controller.abort()
    if (timer)
      clearTimeout(timer)
  })
  return { state, draft, usage, preview, confirming, busy, refreshing, loading, error, usageError, dirty, running, selected, refreshUsage, save, requestCleanup, confirmCleanup, stop }
}
