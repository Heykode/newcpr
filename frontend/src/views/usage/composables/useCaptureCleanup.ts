import type { CleanupJob } from '@/api/modules/log-cleanup'
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { cancelCleanup, getCleanupState, startCaptureCleanup } from '@/api/modules/log-cleanup'
import { errorMessage } from '@/utils/async'

export function useCaptureCleanup(onFinished: () => void) {
  const job = ref<CleanupJob | null>(null)
  const busy = ref(false)
  const error = ref('')
  const running = computed(() => job.value?.captureOnly === true && job.value.status === 'running')
  const occupied = computed(() => job.value?.status === 'running')
  const notice = computed(() => {
    const current = job.value
    if (!current?.captureOnly)
      return ''
    const label = { running: '后台清理中', succeeded: '清理完成', cancelled: '清理已停止', failed: '部分材料清理失败' }[current.status]
    return `${label}，已移除 ${current.removed} 条采集材料`
  })
  let alive = true
  let sequence = 0
  let timer: ReturnType<typeof setTimeout> | undefined
  let controller: AbortController | undefined
  async function refresh() {
    const version = ++sequence
    clearTimeout(timer)
    controller?.abort()
    controller = new AbortController()
    try {
      const state = await getCleanupState({ signal: controller.signal, silent: true })
      if (!alive || version !== sequence)
        return
      const previous = job.value
      job.value = state.job
      error.value = ''
      if (previous?.captureOnly && previous.status === 'running' && (previous.id !== state.job?.id || state.job.status !== 'running'))
        onFinished()
    }
    catch (cause) {
      if (alive && version === sequence)
        error.value = errorMessage(cause, '清理进度读取失败')
    }
    finally {
      if (alive && version === sequence)
        timer = setTimeout(() => void refresh(), occupied.value ? 2000 : 10000)
    }
  }
  async function start() {
    if (busy.value || occupied.value)
      return
    busy.value = true
    error.value = ''
    // Fence a status read that started before the mutation.
    sequence++
    controller?.abort()
    clearTimeout(timer)
    try {
      const accepted = await startCaptureCleanup()
      if (alive)
        job.value = accepted
    }
    catch (cause) {
      if (alive)
        error.value = errorMessage(cause, '清理启动结果未确认，请刷新进度')
    }
    finally {
      if (alive) {
        busy.value = false
        timer = setTimeout(() => void refresh(), 2000)
      }
    }
  }
  async function stop() {
    if (busy.value || !running.value || !job.value)
      return
    busy.value = true
    const id = job.value.id
    try {
      await cancelCleanup(id)
      if (alive) {
        await refresh()
        onFinished()
      }
    }
    catch (cause) {
      if (alive)
        error.value = errorMessage(cause, '停止清理未确认，请刷新进度')
    }
    finally { busy.value = false }
  }
  onMounted(() => void refresh())
  onBeforeUnmount(() => {
    alive = false
    sequence++
    controller?.abort()
    clearTimeout(timer)
  })
  return { job, busy, error, running, occupied, notice, start, stop, refresh }
}
