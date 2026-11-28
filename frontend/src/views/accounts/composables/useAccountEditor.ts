import type { Ref } from 'vue'
import type { AccountModelAccess, getAccounts } from '@/api'
import type { AccountEgressDraft, AccountEgressReadState } from '@/utils/account-egress'
import { onScopeDispose, ref, shallowRef, watch } from 'vue'

import { updateAccount } from '@/api'
import { getIpv6Egress } from '@/api/modules/ipv6-egress'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { accountEgressFromAccount, accountEgressPatch, sameAccountEgress } from '@/utils/account-egress'
import { normalizeAccountName } from '@/utils/account-name'
import { accountModelAccessError } from '../utils/modelAccess'
import { purchaseCostPatch, trimAmount } from '../utils/purchaseCost'
import { concurrencyLimitInput, parseAccountSchedulingForm } from '../utils/schedulingForm'

type AccountRow = Awaited<ReturnType<typeof getAccounts>>['items'][number]

export function useAccountEditor(options: {
  credentialEgress?: boolean
  accounts: Ref<AccountRow[]>
  reloadAccounts: () => Promise<unknown>
  reloadGroups: () => Promise<unknown>
}) {
  const showEditModal = shallowRef(false)
  const editingAccountId = shallowRef<string | null>(null)
  const customName = shallowRef('')
  const purchaseAmount = shallowRef('')
  const purchaseCycleStart = shallowRef('')
  let initialPurchase = ''
  let initialCustomName = ''
  const schedulingEnabled = shallowRef(true)
  const requestProxySource = shallowRef<import('@/utils/request-proxy-source').RequestProxySource>('account')
  const concurrencyLimit = shallowRef('')
  const weight = shallowRef('1')
  const modelAccess = ref<AccountModelAccess | undefined>()
  let initialModelAccess = ''
  const proxyMode = shallowRef('')
  const proxyId = shallowRef('')
  const credentialProxyId = shallowRef<string | null>(null)
  const egressMode = shallowRef('fixed_ipv6_reuse')
  const egressReadState = shallowRef<AccountEgressReadState>({ loading: false, error: false })
  let initialEgress: AccountEgressDraft | undefined
  let egressController: AbortController | undefined
  const selectedGroupIds = ref<string[]>([])
  const saveAction = useAsyncAction()
  const saving = saveAction.loading
  const editingAccount = shallowRef<AccountRow | null>(null)

  function applyInitialEgress(draft: AccountEgressDraft | undefined) {
    initialEgress = draft
    proxyMode.value = draft?.proxyMode ?? ''
    proxyId.value = draft?.proxyId ?? ''
    egressMode.value = draft?.egressMode ?? 'fixed_ipv6_reuse'
  }

  async function loadEgress(account: AccountRow) {
    egressController?.abort()
    applyInitialEgress(accountEgressFromAccount(account))
    egressReadState.value = { loading: account.provider === 'openai', error: false }
    if (account.provider !== 'openai')
      return
    const owner = new AbortController()
    egressController = owner
    try {
      const config = await getIpv6Egress({ silent: true, signal: owner.signal })
      if (owner.signal.aborted || !showEditModal.value || editingAccountId.value !== account.id)
        return
      const draft = accountEgressFromAccount(account, config)
      egressReadState.value = { config, loading: false, error: !draft }
      if (!initialEgress && proxyMode.value === '')
        applyInitialEgress(draft)
    }
    catch {
      if (!owner.signal.aborted)
        egressReadState.value = { loading: false, error: true }
    }
  }

  watch(showEditModal, (open) => {
    if (!open)
      egressController?.abort()
  }, { flush: 'sync' })
  onScopeDispose(() => egressController?.abort())

  function open(account: AccountRow) {
    purchaseAmount.value = trimAmount(account.purchaseCost?.amountCny ?? '')
    purchaseCycleStart.value = account.purchaseCost?.cycleAnchor ?? ''
    initialPurchase = JSON.stringify([purchaseAmount.value, purchaseCycleStart.value])
    editingAccountId.value = account.id
    editingAccount.value = { ...account }
    credentialProxyId.value = account.credentialProxyId ?? null
    customName.value = account.customName ?? ''
    initialCustomName = customName.value
    schedulingEnabled.value = account.enabled
    requestProxySource.value = account.requestProxySource ?? 'account'
    concurrencyLimit.value = concurrencyLimitInput(account.concurrencyLimit)
    weight.value = String(account.weight)
    modelAccess.value = account.modelAccess
      ? { ...account.modelAccess, models: [...account.modelAccess.models] }
      : { mode: 'all', models: [] }
    initialModelAccess = JSON.stringify(modelAccess.value)
    selectedGroupIds.value = account.groups.map(group => group.id)
    showEditModal.value = true
    void loadEgress({ ...account })
  }

  async function save() {
    const accountId = editingAccountId.value
    if (!accountId || saving.value)
      return
    const modelError = accountModelAccessError(modelAccess.value)
    if (modelError) {
      toast.warning(modelError)
      return
    }
    const scheduling = parseAccountSchedulingForm(concurrencyLimit.value, weight.value)
    if (!scheduling.valid) {
      toast.warning(scheduling.message)
      return
    }

    await saveAction.run(async () => {
      const egress = { proxyMode: proxyMode.value, proxyId: proxyId.value, egressMode: egressMode.value }
      if (!initialEgress && egress.proxyMode !== '')
        throw new Error('出口配置尚未读取成功，请关闭后重新打开编辑')
      const payload: Parameters<typeof updateAccount>[0] = {
        accountId,
        ...(options.credentialEgress && editingAccount.value?.provider === 'openai' && credentialProxyId.value !== (editingAccount.value.credentialProxyId ?? null) ? { credentialProxyId: credentialProxyId.value } : {}),
        ...(initialEgress && !sameAccountEgress(initialEgress, egress)
          ? accountEgressPatch(egress, editingAccount.value?.provider === 'openai')
          : {}),
        enabled: schedulingEnabled.value,
        concurrencyLimit: scheduling.values.concurrencyLimit,
        weight: scheduling.values.weight,
        groupIds: [...new Set(selectedGroupIds.value)],
      }
      const name = normalizeAccountName(customName.value)
      if (JSON.stringify([purchaseAmount.value, purchaseCycleStart.value]) !== initialPurchase)
        payload.purchaseCost = purchaseCostPatch(purchaseAmount.value, purchaseCycleStart.value)
      if (name !== normalizeAccountName(initialCustomName))
        payload.customName = name
      if (modelAccess.value && JSON.stringify(modelAccess.value) !== initialModelAccess)
        payload.modelAccess = { ...modelAccess.value, models: [...modelAccess.value.models] }
      await updateAccount(payload)
      showEditModal.value = false
      toast.success('账号已更新')
      void Promise.all([options.reloadAccounts(), options.reloadGroups()]).catch(() => undefined)
    })
  }

  function clear() {
    if (showEditModal.value)
      return
    editingAccountId.value = null
    editingAccount.value = null
    purchaseAmount.value = ''
    purchaseCycleStart.value = ''
    initialPurchase = ''
    customName.value = ''
    initialCustomName = ''
    applyInitialEgress(undefined)
    egressReadState.value = { loading: false, error: false }
    schedulingEnabled.value = true
    requestProxySource.value = 'account'
    concurrencyLimit.value = ''
    weight.value = '1'
    modelAccess.value = undefined
    initialModelAccess = ''
    selectedGroupIds.value = []
  }

  return {
    purchaseAmount,
    purchaseCycleStart,
    customName,
    showEditModal,
    editingAccount,
    schedulingEnabled,
    requestProxySource,
    concurrencyLimit,
    weight,
    modelAccess,
    proxyMode,
    proxyId,
    credentialProxyId,
    egressMode,
    egressReadState,
    selectedGroupIds,
    saving,
    open,
    save,
    clear,
  }
}
