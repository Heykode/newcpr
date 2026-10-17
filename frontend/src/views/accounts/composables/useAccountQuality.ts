import type { AccountRow } from '../constants'
import { onScopeDispose, ref, shallowRef } from 'vue'
import { getQualityMonitoring, setQualityEnabled } from '@/api/modules/quality-ops'
import { toast } from '@/components/base/BaseToast'
import { useIdSet } from '@/composables/useIdSet'
import { errorMessage } from '@/utils/async'

export function useAccountQuality(reload: () => Promise<unknown>) {
  const busy = useIdSet<string>()
  const setupOpen = ref(false)
  const setupAccount = shallowRef<AccountRow | null>(null)
  const reads = new Set<AbortController>()
  let disposed = false

  async function toggle(account: AccountRow) {
    // Capture the intended state before reading: a concurrent edit must not
    // accidentally turn a requested pause into an enable.
    const enabled = !account.qualityMonitoring?.enabled
    await busy.run(account.id, async () => {
      const controller = new AbortController()
      reads.add(controller)
      try {
        const state = await getQualityMonitoring([account.id], { signal: controller.signal, silent: true })
        if (disposed)
          return
        const current = state[account.id]
        if (!current) {
          if (!enabled)
            throw new Error('检测规则已删除，请刷新后重试')
          setupAccount.value = account
          setupOpen.value = true
          return
        }
        if (current.enabled !== enabled)
          await setQualityEnabled({ id: current.ruleId, revision: current.revision, enabled })
        if (!disposed)
          toast.success(enabled ? '质量检测已开启' : '质量检测已关闭')
      }
      catch (cause) {
        if (!disposed)
          toast.error(errorMessage(cause, '质量检测开关更新失败'))
      }
      finally {
        reads.delete(controller)
        if (!disposed)
          await reload().catch(() => undefined)
      }
    })
  }

  onScopeDispose(() => {
    disposed = true
    for (const controller of reads)
      controller.abort()
    reads.clear()
  })
  return { togglingQualityIds: busy.ids, setupOpen, setupAccount, toggle }
}
