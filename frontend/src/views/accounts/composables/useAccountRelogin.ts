import type { Ref } from 'vue'
import type { AccountRow } from '../constants'
import type { AccountReloginAction } from '@/api/modules/relogin'
import { computed, onScopeDispose, shallowRef, watch } from 'vue'
import { getAccountReloginActions, queueAccountRelogin } from '@/api/modules/relogin'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { errorMessage } from '@/utils/async'

export function useAccountRelogin(options: { accounts: Ref<AccountRow[]>, reload: () => Promise<unknown> }) {
  const actions = shallowRef<Record<string, AccountReloginAction>>({})
  const readError = shallowRef('')
  const actionError = shallowRef('')
  const open = shallowRef(false)
  const selected = shallowRef<{ email: string, action: AccountReloginAction } | null>(null)
  const submit = useAsyncAction()
  const ids = computed(() => options.accounts.value.map(account => account.id))
  let disposed = false
  let controller: AbortController | undefined
  let timer: ReturnType<typeof setTimeout> | undefined

  const confirmDisabled = computed(() => {
    const snapshot = selected.value?.action
    const current = snapshot && actions.value[snapshot.accountId]
    return Boolean(readError.value || !snapshot?.target || !current || current.busy
      || current.blockedReason || current.entryId !== snapshot.entryId || current.revision !== snapshot.revision
      || JSON.stringify(current.target) !== JSON.stringify(snapshot.target))
  })

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
        if (previous && ((previous.busy && !action.busy) || previous.syncedAt !== action.syncedAt)) {
          changed = true
          if (previous.busy && !action.busy) {
            const email = options.accounts.value.find(account => account.id === action.accountId)?.email ?? action.accountId
            if (action.status === 'ready' && action.syncedAt)
              toast.success(`${email}：凭据已同步`)
            else if (action.status === 'failed' || action.status === 'uncertain')
              toast.error(`${email}：${action.message}`)
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
        timer = setTimeout(() => void refresh(), readError.value ? 5000 : Object.values(actions.value).some(action => action.busy) ? 2000 : 15000)
    }
  }

  function request(account: AccountRow) {
    const action = actions.value[account.id]
    if (disposed || submit.loading.value || readError.value || !action?.target || action.busy || action.blockedReason)
      return
    selected.value = { email: account.email ?? account.name, action: { ...action, target: { ...action.target } } }
    actionError.value = ''
    open.value = true
  }

  async function confirm() {
    const snapshot = selected.value?.action
    if (disposed || !open.value || confirmDisabled.value || !snapshot?.target)
      return
    const target = snapshot.target
    await submit.run(async () => {
      actionError.value = ''
      await queueAccountRelogin({ entryId: snapshot.entryId, revision: snapshot.revision, target })
      if (disposed)
        return
      open.value = false
      // Keep an accepted job busy until the first authoritative status read.
      actions.value = { ...actions.value, [snapshot.accountId]: { ...snapshot, busy: true, status: 'queued', syncedAt: null } }
      toast.success('已加入重登队列')
      void refresh()
    }, {
      onError: (cause) => {
        if (!disposed) {
          actionError.value = errorMessage(cause, '提交失败，请检查重登状态')
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
  return { actions, readError, actionError, open, selected, confirmDisabled, submitting: submit.loading, refresh, request, confirm }
}
