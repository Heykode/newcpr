import type { AccountImportTask, AccountImportTaskDetail } from '@/api'
import { computed, onMounted, onScopeDispose, shallowRef, watch } from 'vue'
import { getAccountImportTask, getAccountImportTasks, stopAccountImportTask } from '@/api'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { errorMessage } from '@/utils/async'

export function useAccountImportTasks(options: { reload: () => Promise<unknown> }) {
  const open = shallowRef(false)
  const tasks = shallowRef<AccountImportTask[]>([])
  const selectedId = shallowRef('')
  const detail = shallowRef<AccountImportTaskDetail | null>(null)
  const error = shallowRef('')
  const stopError = shallowRef('')
  const loading = shallowRef(false)
  const stopAction = useAsyncAction()
  const activeCount = computed(() => tasks.value.filter(task => !task.finishedAt).length)
  let timer: ReturnType<typeof setTimeout> | undefined
  let controller: AbortController | undefined
  let disposed = false
  let initialized = false
  let selectionVersion = 0

  function stopConfirmed(task: AccountImportTaskDetail | null, taskId: string) {
    return task?.taskId === taskId && Boolean(task.stopRequested || task.finishedAt)
  }

  function acceptDetail(next: AccountImportTaskDetail, taskId: string) {
    if (selectedId.value !== taskId || next.taskId !== taskId)
      return
    detail.value = next
    if (stopConfirmed(next, taskId))
      stopError.value = ''
  }

  async function refresh(background = false) {
    if (disposed)
      return
    clearTimeout(timer)
    controller?.abort()
    const current = new AbortController()
    controller = current
    loading.value = !background
    try {
      const result = await getAccountImportTasks({ silent: true, signal: current.signal })
      if (current.signal.aborted)
        return
      const previous = new Map(tasks.value.map(task => [task.taskId, task.counts.importedAccounts]))
      // 首次读取只建立基线，历史结果不重复触发账号页的初始加载。
      const changed = initialized && result.items.some(task => task.counts.importedAccounts > (previous.get(task.taskId) ?? 0))
      initialized = true
      tasks.value = result.items
      if (changed)
        void options.reload().catch(() => undefined)
      if (!tasks.value.some(task => task.taskId === selectedId.value)) {
        detail.value = null
        stopError.value = ''
        selectedId.value = tasks.value[0]?.taskId ?? ''
      }
      if (open.value && selectedId.value) {
        const selected = selectedId.value
        const next = await getAccountImportTask({ taskId: selected }, { silent: true, signal: current.signal })
        if (!current.signal.aborted)
          acceptDetail(next, selected)
      }
      if (!current.signal.aborted)
        error.value = ''
    }
    catch (cause) {
      if (!current.signal.aborted)
        error.value = errorMessage(cause, '进度读取失败，请重试')
    }
    finally {
      if (!current.signal.aborted && !disposed) {
        loading.value = false
        // 打开时发现其他页面提交的任务；初次断线也持续重试只读查询。
        if (open.value || activeCount.value > 0 || error.value)
          timer = setTimeout(() => void refresh(true), error.value ? 5000 : 1500)
      }
    }
  }

  function select(taskId: string) {
    selectedId.value = taskId
    detail.value = null
    stopError.value = ''
    void refresh()
  }

  function created(task: AccountImportTask) {
    if (disposed)
      return
    const previous = tasks.value.find(entry => entry.taskId === task.taskId)
    if (task.counts.importedAccounts > (previous?.counts.importedAccounts ?? 0))
      void options.reload().catch(() => undefined)
    initialized = true
    tasks.value = [task, ...tasks.value.filter(entry => entry.taskId !== task.taskId)]
    selectedId.value = task.taskId
    detail.value = null
    stopError.value = ''
    if (open.value)
      void refresh()
    else
      open.value = true
  }

  async function stop() {
    const task = detail.value
    if (disposed || !task || task.taskId !== selectedId.value || task.finishedAt || task.stopRequested || task.counts.pending === 0)
      return
    const version = selectionVersion
    await stopAction.run(async () => {
      stopError.value = ''
      const result = await stopAccountImportTask({ taskId: task.taskId })
      if (disposed || version !== selectionVersion)
        return
      acceptDetail(result, task.taskId)
      await refresh()
    }, {
      onError: (cause) => {
        if (!disposed && version === selectionVersion && selectedId.value === task.taskId && !stopConfirmed(detail.value, task.taskId))
          stopError.value = errorMessage(cause, '停止请求失败，请重试')
      },
    })
  }

  watch(selectedId, () => {
    selectionVersion++
  }, { flush: 'sync' })
  watch(open, () => void refresh())
  onMounted(() => void refresh())
  onScopeDispose(() => {
    disposed = true
    controller?.abort()
    clearTimeout(timer)
    loading.value = false
  })

  return { open, tasks, detail, selectedId, error, stopError, loading, activeCount, stopping: stopAction.loading, select, created, refresh, stop }
}
