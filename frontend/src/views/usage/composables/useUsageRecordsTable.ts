import type { Ref } from 'vue'
import type { UsageDisplayRecord } from '../utils/records'
import type { UsageTimeRangeParams } from './useUsageTimeRange'
import type { UsageFilterParams } from '@/api'
import { watchDebounced } from '@vueuse/core'

import { computed, onMounted, onScopeDispose, shallowRef, watch } from 'vue'
import {
  getUsageRecords,
} from '@/api'
import { withMinimumDuration } from '@/utils/async'
import { usageSearchParam } from '../utils/search'
import { useUsageInsights } from './useUsageInsights'

interface UseUsageRecordsTableOptions {
  timeRangeParams: Readonly<Ref<UsageTimeRangeParams>>
  latestTimeRangeParams: () => UsageTimeRangeParams
  active: Readonly<Ref<boolean>>
  filters?: Readonly<Ref<UsageFilterParams>>
  provider?: Ref<string>
  recordStatus?: Readonly<Ref<'success' | 'error' | 'all'>>
}

type UsageLoadScope = 'all' | 'table'

interface UsageLoadOptions {
  scope?: UsageLoadScope
  background?: boolean
}

export function useUsageRecordsTable(options: UseUsageRecordsTableOptions) {
  const loading = shallowRef(true)
  const error = shallowRef('')
  const records = shallowRef<UsageDisplayRecord[]>([])
  const currentPage = shallowRef(1)
  const pageSize = shallowRef(10)
  const totalRecords = shallowRef(0)
  const searchQuery = shallowRef('')
  const search = computed(() => usageSearchParam(searchQuery.value))
  const providerQuery = options.provider ?? shallowRef('')
  let tableParams = snapshot()
  const refreshingList = shallowRef(false)
  let tableRequestId = 0
  let tableController: AbortController | undefined
  let disposed = false
  const filterKey = () => JSON.stringify({
    ...options.timeRangeParams.value,
    provider: providerQuery.value || undefined,
    search: search.value,
    ...options.filters?.value,
  })
  let tableFilterKey = filterKey()
  const scopedParams = () => tableParams
  const analytics = useUsageInsights(scopedParams)
  const { loadUsageAnalytics } = analytics

  function handleDiagnosticPageChange(page: number) {
    if (tableFilterKey !== filterKey())
      return
    analytics.handleDiagnosticPageChange(page)
  }
  const usagePagination = computed(() => ({
    currentPage: currentPage.value,
    pageSize: pageSize.value,
    total: totalRecords.value,
  }))

  function snapshot() {
    return {
      ...options.latestTimeRangeParams(),
      provider: providerQuery.value || undefined,
      search: search.value,
      ...options.filters?.value,
    }
  }

  function resetPagination() {
    currentPage.value = 1
    totalRecords.value = 0
  }

  async function loadUsageRecords(loadOptions: UsageLoadOptions = {}) {
    const { scope = 'all', background = false } = loadOptions
    if (scope === 'all') {
      resetPagination()
      tableParams = snapshot()
      tableFilterKey = filterKey()
      records.value = []
    }

    await Promise.all([
      ...(options.active.value ? [loadUsagePage(background)] : []),
      ...(scope === 'all' ? [loadUsageAnalytics(tableParams)] : []),
    ])
  }

  async function loadUsagePage(background: boolean) {
    const requestId = ++tableRequestId
    tableController?.abort()
    tableController = new AbortController()
    error.value = ''
    loading.value = !background
    try {
      const result = await getUsageRecords({
        recordStatus: options.recordStatus?.value,
        currentPage: currentPage.value,
        pageSize: pageSize.value,
        ...tableParams,
      }, { signal: tableController.signal })
      if (requestId !== tableRequestId)
        return

      records.value = result.items
      pageSize.value = result.pageSize
      totalRecords.value = result.total
      currentPage.value = result.currentPage
    }
    catch {
      if (requestId === tableRequestId)
        error.value = '请求明细加载失败，请重试'
    }
    finally {
      if (requestId === tableRequestId) {
        loading.value = false
      }
    }
  }

  async function refreshUsageRecords() {
    if (refreshingList.value || loading.value)
      return
    refreshingList.value = true
    try {
      await withMinimumDuration(reloadLatestTable)
    }
    finally {
      refreshingList.value = false
    }
  }

  function reloadLatestTable() {
    return loadUsageRecords()
  }

  function handlePageChange(nextPage: number) {
    if (tableFilterKey !== filterKey()) {
      void reloadLatestTable()
      return
    }
    currentPage.value = nextPage
    void loadUsageRecords({ scope: 'table' })
  }

  function handlePageSizeChange(nextPageSize: number) {
    pageSize.value = nextPageSize
    if (tableFilterKey !== filterKey()) {
      void reloadLatestTable()
      return
    }
    resetPagination()
    void loadUsageRecords({ scope: 'table' })
  }

  onMounted(() => {
    loadUsageRecords()
  })

  watch(() => options.recordStatus?.value, () => {
    resetPagination()
    if (options.active.value)
      void loadUsageRecords({ scope: 'table' })
  })

  watch(providerQuery, () => {
    void loadUsageRecords({ background: true })
  })

  watch(options.active, (active) => {
    if (active) {
      if (tableFilterKey !== filterKey())
        void reloadLatestTable()
      else
        void loadUsageRecords({ scope: 'table' })
    }
    else {
      tableRequestId += 1
      tableController?.abort()
    }
  })

  watchDebounced(
    () => JSON.stringify([search.value, options.filters?.value]),
    () => {
      if (!disposed && tableFilterKey !== filterKey())
        void reloadLatestTable()
    },
    { debounce: 250 },
  )

  watch(() => JSON.stringify([search.value, options.filters?.value]), () => {
    ++tableRequestId
    tableController?.abort()
    analytics.invalidate()
    records.value = []
    loading.value = true
  }, { flush: 'sync' })

  onScopeDispose(() => {
    disposed = true
    tableRequestId += 1
    tableController?.abort()
    analytics.invalidate()
  })

  return {
    currentPage,
    pageSize,
    searchQuery,
    providerQuery,
    usagePagination,
    loading,
    error,
    records,
    ...analytics,
    refreshingList,
    loadUsageRecords,
    refreshUsageRecords,
    handlePageChange,
    handlePageSizeChange,
    handleDiagnosticPageChange,
  }
}
