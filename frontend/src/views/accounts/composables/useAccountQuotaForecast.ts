import type { Ref } from 'vue'
import type { AccountForecastCache } from './useAccountForecastCache'
import type { Account, AccountQuotaForecastResponse } from '@/api'
import { onScopeDispose, shallowRef, watch } from 'vue'
import { getAccountQuotaForecast, refreshAccountQuota } from '@/api'
import { toast } from '@/components/base/BaseToast'

export function useAccountQuotaForecast(
  accountId: Ref<string>,
  open: Ref<boolean>,
  onAccountUpdated: (account: Account) => void,
  cache?: AccountForecastCache,
) {
  const report = shallowRef<AccountQuotaForecastResponse | null>(null)
  const loading = shallowRef(false)
  const refreshing = shallowRef(false)
  const error = shallowRef(false)
  let requestVersion = 0
  let controller: AbortController | undefined
  let disposed = false

  function cancelLoad() {
    requestVersion += 1
    controller?.abort()
    controller = undefined
    loading.value = false
  }

  async function load() {
    if (!open.value || !accountId.value || disposed)
      return false
    cancelLoad()
    const version = requestVersion
    controller = new AbortController()
    loading.value = true
    error.value = false
    try {
      const result = cache
        ? await cache.read(accountId.value, controller.signal)
        : await getAccountQuotaForecast({ accountId: accountId.value }, { signal: controller.signal })
      if (version === requestVersion) {
        report.value = result
        error.value = result === null
        return result !== null
      }
    }
    catch {
      if (version === requestVersion) {
        error.value = true
      }
    }
    finally {
      if (version === requestVersion)
        loading.value = false
    }
    return false
  }

  async function refresh() {
    if (refreshing.value || !open.value)
      return
    const targetAccountId = accountId.value
    cancelLoad()
    refreshing.value = true
    error.value = false
    try {
      // 刷新属于现有额度动作；预测查询本身始终只读。
      const result = await refreshAccountQuota({ accountId: targetAccountId })
      if (disposed)
        return
      cache?.invalidate(targetAccountId)
      cache?.syncAccount(result.account)
      onAccountUpdated(result.account)
      if (accountId.value === targetAccountId && await load())
        toast.success('额度已刷新')
    }
    catch {
      if (!disposed && accountId.value === targetAccountId) {
        if (!report.value)
          error.value = true
      }
    }
    finally {
      refreshing.value = false
    }
  }

  function retry() {
    cache?.invalidate(accountId.value)
    return load()
  }

  watch([open, accountId], ([isOpen]) => {
    cancelLoad()
    report.value = null
    error.value = false
    if (isOpen)
      void load()
  }, { immediate: true })

  watch(() => open.value ? cache?.peek(accountId.value, Date.now()) : null, (latest) => {
    if (latest && !disposed && latest.accountId === accountId.value) {
      report.value = latest
      error.value = false
    }
  })

  onScopeDispose(() => {
    disposed = true
    cancelLoad()
  })

  return { report, loading, refreshing, error, load: retry, refresh }
}
