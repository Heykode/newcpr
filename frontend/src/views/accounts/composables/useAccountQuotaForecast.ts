import type { Ref } from 'vue'
import type { AccountForecastCache } from './useAccountForecastCache'
import type { Account, AccountQuotaForecastResponse } from '@/api'
import { onScopeDispose, shallowRef, watch } from 'vue'
import { getAccountQuotaForecast, refreshAccountQuota } from '@/api'
import { toast } from '@/components/base/BaseToast'
import { useRequestState } from '@/composables/useRequestState'
import { errorMessage } from '@/utils/async'

export function useAccountQuotaForecast(
  accountId: Ref<string>,
  open: Ref<boolean>,
  onAccountUpdated: (account: Account) => void,
  cache?: AccountForecastCache,
) {
  const report = shallowRef<AccountQuotaForecastResponse | null>(null)
  const request = useRequestState()
  const { loading, error } = request
  const refreshing = shallowRef(false)
  let disposed = false

  async function load() {
    if (!open.value || !accountId.value || disposed)
      return false
    const requestId = request.start()
    try {
      const result = cache
        ? await cache.read(accountId.value, request.signal)
        : await getAccountQuotaForecast({ accountId: accountId.value }, { signal: request.signal })
      if (request.isCurrent(requestId)) {
        report.value = result
        error.value = result === null ? '额度预测暂不可用' : ''
        return result !== null
      }
    }
    catch (cause) {
      request.fail(requestId, cause)
    }
    finally {
      request.finish(requestId)
    }
    return false
  }

  async function refresh() {
    if (refreshing.value || !open.value)
      return
    const targetAccountId = accountId.value
    request.invalidate()
    refreshing.value = true
    error.value = ''
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
    catch (cause) {
      if (!disposed && accountId.value === targetAccountId) {
        if (!report.value)
          error.value = errorMessage(cause)
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
    request.invalidate({ resetLoading: isOpen })
    if (!isOpen)
      return
    report.value = null
    error.value = ''
    if (isOpen)
      void load()
  }, { immediate: true, flush: 'sync' })

  watch(() => open.value ? cache?.peek(accountId.value, Date.now()) : null, (latest) => {
    if (latest && !disposed && latest.accountId === accountId.value) {
      report.value = latest
      error.value = ''
    }
  })

  onScopeDispose(() => {
    disposed = true
  })

  return { report, loading, refreshing, error, load: retry, refresh }
}
