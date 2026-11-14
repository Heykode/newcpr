import type { Ref } from 'vue'
import type { AccountRow } from '../constants'
import type { AccountReloginAction, CredentialSelection, CredentialTarget } from '@/api/modules/relogin'
import { computed, onScopeDispose, shallowRef, watch } from 'vue'
import { getAccountReloginActions, queueAccountRelogin } from '@/api/modules/relogin'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { errorMessage } from '@/utils/async'

export function useAccountRelogin(options: { accounts: Ref<AccountRow[]>, reload: () => Promise<unknown> }) {
  const actions = shallowRef<Record<string, AccountReloginAction>>({})
  const readError = shallowRef('')
  const actionError = shallowRef('')
  const submit = useAsyncAction()
  const ids = computed(() => options.accounts.value.map(account => account.id))
  let disposed = false
  let controller: AbortController | undefined
  let timer: ReturnType<typeof setTimeout> | undefined

  async function refresh() {
    if (disposed)
      return
    clearTimeout(timer)
    controller?.abort()
    const current = new AbortController()
    controller = current
    const requested = ids.value
    if (!requested.length) {
      actions.value = {}
      readError.value = ''
      return
    }
    try {
      const result = await getAccountReloginActions(requested, { silent: true, signal: current.signal })
      if (disposed || current.signal.aborted)
        return
      let changed = false
      for (const action of result) {
        const previous = actions.value[action.accountId]
        for (const target of ['codex', 'excel'] as const) {
          const before = target === 'codex' ? previous : previous?.excel
          const after = target === 'codex' ? action : action.excel
          if (!before || !after || (!(before.busy && !after.busy) && before.syncedAt === after.syncedAt))
            continue
          changed = true
          if (before.busy && !after.busy) {
            const email = options.accounts.value.find(account => account.id === action.accountId)?.email ?? action.accountId
            const label = target === 'excel' ? 'Excel' : 'Codex'
            if (after.status === 'ready' && after.syncedAt)
              toast.success(`${email} ${label}：凭据已同步`)
            else if (after.status === 'failed' || after.status === 'uncertain')
              toast.error(`${email} ${label}：${after.message}`)
          }
        }
      }
      actions.value = Object.fromEntries(result.map(action => [action.accountId, action]))
      readError.value = ''
      if (changed)
        void options.reload().catch(() => undefined)
    }
    catch (cause) {
      if (!disposed && !current.signal.aborted)
        readError.value = errorMessage(cause, '重登状态读取失败')
    }
    finally {
      if (!disposed && !current.signal.aborted)
        timer = setTimeout(() => void refresh(), readError.value ? 5000 : Object.values(actions.value).some(action => action.busy || action.excel?.busy) ? 2000 : 15000)
    }
  }

  async function request(account: AccountRow, selection: CredentialSelection = account.responsesUpstream === 'excel' ? 'excel' : 'codex') {
    const action = actions.value[account.id]
    if (disposed || submit.loading.value || readError.value || !action)
      return
    const targets: CredentialTarget[] = selection === 'all' ? ['codex', 'excel'] : [selection]
    const channels = targets.map(target => target === 'codex' ? action : action.excel)
    if (channels.some(channel => !channel?.target || channel.busy || channel.blockedReason))
      return
    const snapshots = channels.map(channel => ({ ...channel!, target: { ...channel!.target! } }))
    const first = snapshots[0]!
    await submit.run(async () => {
      actionError.value = ''
      await queueAccountRelogin({ entryId: first.entryId, revision: action.revision, target: first.target, additionalTarget: snapshots[1]?.target })
      if (disposed)
        return
      let next = { ...action }
      for (const target of targets) {
        if (target === 'codex')
          next = { ...next, busy: true, status: 'queued' }
        else if (next.excel)
          next.excel = { ...next.excel, busy: true, status: 'queued' }
      }
      actions.value = { ...actions.value, [account.id]: next }
      toast.success('已加入重登队列，成功后自动推送')
      void refresh()
    }, {
      onError: (cause) => {
        if (!disposed) {
          actionError.value = errorMessage(cause, '提交失败，请检查重登状态')
          toast.error(actionError.value)
          void refresh()
        }
      },
    })
  }

  watch(() => JSON.stringify(ids.value), () => {
    actions.value = {}
    void refresh()
  }, { immediate: true, flush: 'sync' })
  onScopeDispose(() => {
    disposed = true
    controller?.abort()
    clearTimeout(timer)
  })
  return { actions, readError, actionError, submitting: submit.loading, refresh, request }
}
