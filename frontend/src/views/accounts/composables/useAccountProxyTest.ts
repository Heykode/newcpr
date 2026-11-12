import type { OutboundProxyRecord, OutboundProxyTest } from '@/api'
import { computed, shallowRef, watch } from 'vue'
import { probeProxy, testProxy } from '@/api'
import { ApiError } from '@/api/request'
import { toast } from '@/components/base/BaseToast'
import { useRequestState } from '@/composables/useRequestState'
import { t } from '@/i18n'
import { errorMessage } from '@/utils/async'

export function useAccountProxyTest(options: {
  target: () => OutboundProxyRecord | 'deployment' | null
  accountId: () => string | undefined
  onTested: (proxy: OutboundProxyRecord) => void
  onConflict: () => Promise<void>
}) {
  const request = useRequestState(cause => toast.error(errorMessage(cause)))
  const result = shallowRef<OutboundProxyTest | null>(null)
  const defaultResult = shallowRef<OutboundProxyTest | null>(null)
  const targetId = computed(() => {
    const target = options.target()
    return typeof target === 'object'
      ? target?.id
      : target
  })

  watch([targetId, options.accountId], ([, accountId], [, previousAccountId]) => {
    request.invalidate()
    result.value = null
    if (accountId !== previousAccountId)
      defaultResult.value = null
    request.error.value = ''
  }, { flush: 'sync' })

  async function test() {
    const target = options.target()
    if (!target || request.loading.value)
      return
    const requestId = request.start()
    result.value = null
    const requestOptions = { signal: request.signal, silent: true }
    try {
      if (target === 'deployment') {
        const response = await probeProxy({ proxyUrl: '', useDeploymentProxy: true, detectLocation: false }, requestOptions)
        if (request.isCurrent(requestId)) {
          result.value = response
          defaultResult.value = response
        }
      }
      else {
        const response = await testProxy({ id: target.id, revision: target.revision, detectLocation: false }, requestOptions)
        if (!request.isCurrent(requestId))
          return
        options.onTested(response)
        result.value = response.lastTest
      }
      if (result.value && !result.value.success)
        request.fail(requestId, new Error(result.value.message || t('proxies.actions.testFailed')))
    }
    catch (error) {
      if (request.isCurrent(requestId) && error instanceof ApiError && error.status === 409)
        await options.onConflict()
      request.fail(requestId, error)
    }
    finally {
      request.finish(requestId)
    }
  }

  return {
    result,
    defaultResult,
    testing: request.loading,
    test,
  }
}
