import type { Ref } from 'vue'
import type { Account } from '@/api'
import { watch } from 'vue'
import { accountForecastSignature, useAccountForecastCache } from './useAccountForecastCache'

export function useAccountListForecast(accounts: Ref<Account[]>) {
  const cache = useAccountForecastCache()
  watch(
    () => JSON.stringify(accounts.value.map(account => [account.id, accountForecastSignature(account)])),
    () => {
      for (const account of accounts.value)
        cache.syncAccount(account)
    },
    { immediate: true },
  )
  return { cache }
}
