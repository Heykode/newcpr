import type { AccountRow } from '../constants'
import { onScopeDispose, ref, shallowRef } from 'vue'
import { getQualityMonitoring, setQualityEnabled } from '@/api/modules/quality-ops'
import { toast } from '@/components/base/BaseToast'
import { useIdSet } from '@/composables/useIdSet'
import { errorMessage } from '@/utils/async'

export function useAccountQuality(reload: () => Promise<unknown>) {
  const busy = useIdSet<string>()
  const batchBusy = ref(false)
  const setupOpen = ref(false)
  const setupAccount = shallowRef<AccountRow | null>(null)
  const reads = new Set<AbortController>()
  let disposed = false

  async function toggle(account: AccountRow) {
    if (disposed || batchBusy.value)
      return
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

  async function setBatchEnabled(accountIds: Iterable<string>, enabled: boolean) {
    if (disposed || batchBusy.value || busy.ids.value.size || setupOpen.value)
      return
    const ids = [...new Set(accountIds)]
    if (!ids.length || ids.length > 1000) {
      toast.error('请选择 1–1000 个账号')
      return
    }

    batchBusy.value = true
    const controller = new AbortController()
    reads.add(controller)
    let completed = 0
    let skipped = 0
    let failed = 0
    let firstError = ''
    try {
      const monitoring = await getQualityMonitoring(ids, { signal: controller.signal, silent: true })
      let next = 0
      // Bound writes while retaining a fresh CAS baseline and the captured intent.
      await Promise.all(Array.from({ length: Math.min(4, ids.length) }, async () => {
        while (next < ids.length) {
          if (disposed)
            break
          const accountId = ids[next++]
          if (accountId === undefined)
            break
          const current = monitoring[accountId]
          if (!current) {
            skipped++
            continue
          }
          try {
            if (current.enabled !== enabled) {
              await busy.run(accountId, () => setQualityEnabled({
                id: current.ruleId,
                revision: current.revision,
                enabled,
              }))
            }
            completed++
          }
          catch (cause) {
            failed++
            firstError ||= errorMessage(cause, '请求失败，请刷新核对状态')
          }
        }
      }))
      if (!disposed) {
        const parts = [`质量检测已${enabled ? '开启' : '关闭'} ${completed} 个`]
        if (skipped)
          parts.push(`${skipped} 个未配置，已跳过`)
        if (failed)
          parts.push(`${failed} 个未完成：${firstError}`)
        else if (skipped && enabled)
          parts.push('请先应用监测模板')
        if (failed || skipped)
          toast.warning(parts.join('；'), { duration: 6000 })
        else
          toast.success(parts.join('；'))
      }
    }
    catch (cause) {
      if (!disposed)
        toast.error(errorMessage(cause, '监测状态读取失败，请重试'))
    }
    finally {
      reads.delete(controller)
      if (!disposed) {
        const refreshed = await reload().catch(() => false)
        if (!disposed && refreshed === false)
          toast.warning('账号列表刷新失败，请手动刷新核对状态')
      }
      batchBusy.value = false
    }
  }

  onScopeDispose(() => {
    disposed = true
    for (const controller of reads)
      controller.abort()
    reads.clear()
  })
  return { togglingQualityIds: busy.ids, batchBusy, setupOpen, setupAccount, toggle, setBatchEnabled }
}
