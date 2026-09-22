import { onScopeDispose, shallowReactive, watch } from 'vue'
import { requestAccountTurnStateProbe } from '@/api'
import { toast } from '@/components/base/BaseToast'
import { errorMessage } from '@/utils/async'

export function useTurnStateProbe(options: {
  accountId: () => string
  canProbe: (model: string) => boolean
  onQueued: () => void
}) {
  const pending = shallowReactive(new Set<string>())
  let generation = 0
  watch(options.accountId, () => {
    generation += 1
    pending.clear()
  }, { flush: 'sync' })
  onScopeDispose(() => {
    generation += 1
  })

  async function requestProbe(modelId: string) {
    if (pending.has(modelId) || !options.canProbe(modelId))
      return
    const owner = generation
    const accountId = options.accountId()
    pending.add(modelId)
    try {
      const result = await requestAccountTurnStateProbe({ accountId, modelId }, { silent: true })
      if (owner !== generation)
        return
      toast.success(result.status === 'queued' ? 'State 探测已排队' : '该模型正在排队或采集中')
      options.onQueued()
    }
    catch (error: unknown) {
      if (owner === generation)
        toast.error(errorMessage(error, 'State 探测提交失败'))
    }
    finally {
      if (owner === generation)
        pending.delete(modelId)
    }
  }

  return { pending, requestProbe }
}
