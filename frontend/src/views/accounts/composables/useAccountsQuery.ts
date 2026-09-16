import type { BaseTableSort } from '@/components/base/BaseTable/columns'
import { useIntervalFn, watchDebounced } from '@vueuse/core'

import { computed, onMounted, shallowRef, watch } from 'vue'
import { getAccounts } from '@/api'
import { usePagedQuery } from '@/composables/usePagedQuery'

type AccountRow = Awaited<ReturnType<typeof getAccounts>>['items'][number]

export function useAccountsQuery() {
  const searchQuery = shallowRef('')
  const providerQuery = shallowRef('')
  const statusQuery = shallowRef('')
  const groupQuery = shallowRef('')
  const planTypeQuery = shallowRef('')
  const sort = shallowRef<BaseTableSort>()
  const activeRequests = shallowRef(0)
  const refreshing = computed(() => activeRequests.value > 0)
  const accountSummary = shallowRef({
    total: 0,
    normal: 0,
    quotaExhausted: 0,
    rateLimited: 0,
    disabled: 0,
    error: 0,
  })

  const query = usePagedQuery({
    initialPageSize: 20,
    load: async ({ page, pageSize }, options) => {
      activeRequests.value += 1
      try {
        return await getAccounts({
          page,
          pageSize,
          search: searchQuery.value,
          provider: providerQuery.value || undefined,
          status: statusQuery.value || undefined,
          planType: planTypeQuery.value || undefined,
          groupId: groupQuery.value || undefined,
          sortBy: sort.value?.key ?? 'addedAt',
          sortDirection: sort.value?.direction ?? 'desc',
        }, options)
      }
      finally {
        activeRequests.value -= 1
      }
    },
    onSuccess: (result) => {
      accountSummary.value = result.summary
    },
  })

  const accountPagination = computed(() => ({
    currentPage: query.page.value,
    pageSize: query.pageSize.value,
    total: query.total.value,
  }))

  function handlePageChange(page: number) {
    query.page.value = page
    void query.execute()
  }

  function handlePageSizeChange(pageSize: number) {
    query.pageSize.value = pageSize
    query.page.value = 1
    void query.execute()
  }

  function handleSortChange(nextSort: BaseTableSort | undefined) {
    sort.value = nextSort
    query.page.value = 1
    void query.execute()
  }

  async function replaceAccount(updated: AccountRow) {
    // 状态变更可能影响筛选、排序及全局概览，统一回读并复用分页查询的末页回退。
    query.invalidate()
    if (!await query.execute())
      return true // 回读失败或被新查询取代时，不依据旧页面取消选择。
    return query.items.value.some(account => account.id === updated.id)
  }

  watchDebounced(
    searchQuery,
    () => {
      query.page.value = 1
      void query.execute()
    },
    { debounce: 250 },
  )

  watch([providerQuery, statusQuery, groupQuery], () => {
    query.page.value = 1
    void query.execute()
  })

  watch(planTypeQuery, () => {
    query.page.value = 1
    void query.execute()
  })

  onMounted(() => {
    void query.execute()
  })

  function refreshAccounts() {
    if (refreshing.value)
      return Promise.resolve(false)
    return query.execute()
  }

  useIntervalFn(async () => {
    if (refreshing.value)
      return
    await query.execute({ silent: true })
  }, 30_000)

  return {
    page: query.page,
    pageSize: query.pageSize,
    totalAccounts: query.total,
    loading: query.loading,
    refreshing,
    accounts: query.items,
    loadAccounts: query.execute,
    refreshAccounts,
    refreshAccountsSilently: () => query.execute({ silent: true }),
    searchQuery,
    providerQuery,
    statusQuery,
    groupQuery,
    planTypeQuery,
    sort,
    accountSummary,
    accountPagination,
    replaceAccount,
    handlePageChange,
    handlePageSizeChange,
    handleSortChange,
  }
}
