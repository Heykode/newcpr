import type { Ref } from 'vue'
import type { AccountConcurrencyPage, AccountConcurrencyPool } from '@/api/modules/accounts'
import { useDocumentVisibility } from '@vueuse/core'
import { computed, onScopeDispose, shallowRef, watch } from 'vue'
import { getAccountConcurrency } from '@/api/modules/accounts'

export function useAccountConcurrency(accountId: () => string, open: Ref<boolean>, pool: Ref<AccountConcurrencyPool>) {
  const data = shallowRef<AccountConcurrencyPage | null>(null)
  const loading = shallowRef(false)
  const error = shallowRef('')
  const visibility = useDocumentVisibility()
  const active = computed(() => open.value && visibility.value === 'visible')
  let generation = 0
  let controller: AbortController | undefined
  let timer: ReturnType<typeof setTimeout> | undefined

  function stop() {
    ++generation
    controller?.abort()
    clearTimeout(timer)
    loading.value = false
  }

  async function load() {
    if (!active.value || loading.value)
      return
    clearTimeout(timer)
    const current = ++generation
    controller = new AbortController()
    const signal = controller.signal
    const scope = { accountId: accountId(), pool: pool.value }
    loading.value = true
    error.value = ''
    try {
      const read = (page: number) => getAccountConcurrency({ ...scope, page, pageSize: 100 }, { signal, silent: true })
      const first = await read(1)
      if (current !== generation)
        return
      if (!first) {
        data.value = null
        return
      }
      const collected = new Map<string, AccountConcurrencyPage['items'][number]>()
      const collect = (page: AccountConcurrencyPage) => page.items.forEach((item, index) => {
        collected.set(item.leaseId || `legacy:${page.page}:${index}`, item)
      })
      collect(first)
      // Freeze this refresh's page count; arriving requests cannot extend it indefinitely.
      const pages = Math.ceil(first.total / Math.max(1, first.pageSize))
      for (let page = 2; page <= pages; page++) {
        const next = await read(page)
        if (current !== generation)
          return
        if (!next)
          throw new Error('missing concurrency snapshot')
        collect(next)
        if (!next.items.length)
          break
      }
      data.value = { ...first, items: [...collected.values()] }
    }
    catch {
      if (current === generation)
        error.value = '并发详情刷新失败'
    }
    finally {
      if (current === generation) {
        loading.value = false
        if (active.value)
          timer = setTimeout(load, 5000)
      }
    }
  }
  watch([active, accountId, pool], ([enabled, id, selected], [, previousId, previousPool]) => {
    stop()
    if (!enabled || id !== previousId || selected !== previousPool) {
      data.value = null
      error.value = ''
    }
    if (enabled)
      void load()
  }, { immediate: true })
  onScopeDispose(stop)
  return { data, loading, error, load }
}
