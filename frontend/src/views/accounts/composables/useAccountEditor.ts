import type { Ref } from 'vue'
import type { AccountModelAccess, getAccounts } from '@/api'

import { computed, ref, shallowRef, watch } from 'vue'
import { updateAccount } from '@/api'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { normalizeAccountName } from '@/utils/account-name'
import { accountModelAccessError } from '../utils/modelAccess'
import { concurrencyLimitInput, parseAccountSchedulingForm } from '../utils/schedulingForm'

type AccountRow = Awaited<ReturnType<typeof getAccounts>>['items'][number]

export function useAccountEditor(options: {
  accounts: Ref<AccountRow[]>
  reloadAccounts: () => Promise<unknown>
  reloadGroups: () => Promise<unknown>
}) {
  const showEditModal = shallowRef(false)
  const editingAccountId = shallowRef<string | null>(null)
  const customName = shallowRef('')
  let initialCustomName = ''
  const schedulingEnabled = shallowRef(true)
  const turnStateInjectionEnabled = shallowRef(false)
  const concurrencyLimit = shallowRef('')
  const weight = shallowRef('1')
  const modelAccess = ref<AccountModelAccess | undefined>()
  let initialModelAccess = ''
  const proxyMode = shallowRef('preserve')
  const proxyId = shallowRef('')
  const selectedGroupIds = ref<string[]>([])
  const saveAction = useAsyncAction()
  const saving = saveAction.loading
  const editingAccount = computed(() => {
    const accountId = editingAccountId.value
    return accountId
      ? options.accounts.value.find(account => account.id === accountId) ?? null
      : null
  })

  function open(account: AccountRow) {
    editingAccountId.value = account.id
    customName.value = account.customName ?? ''
    initialCustomName = customName.value
    proxyMode.value = 'preserve'
    proxyId.value = ''
    schedulingEnabled.value = account.enabled
    turnStateInjectionEnabled.value = account.turnStateInjectionEnabled
    concurrencyLimit.value = concurrencyLimitInput(account.concurrencyLimit)
    weight.value = String(account.weight)
    modelAccess.value = account.modelAccess
      ? { ...account.modelAccess, models: [...account.modelAccess.models] }
      : { mode: 'all', models: [] }
    initialModelAccess = JSON.stringify(modelAccess.value)
    selectedGroupIds.value = account.groups.map(group => group.id)
    showEditModal.value = true
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
    if (proxyMode.value === 'proxy' && !proxyId.value.trim()) {
      toast.warning('请选择已通过测试的代理')
      return
    }
    if (!scheduling.valid) {
      toast.warning(scheduling.message)
      return
    }

    await saveAction.run(async () => {
      const payload: Parameters<typeof updateAccount>[0] = {
        accountId,
        outboundProxyId: proxyMode.value === 'preserve' ? undefined : proxyMode.value === 'direct' ? '' : proxyId.value.trim(),
        enabled: schedulingEnabled.value,
        concurrencyLimit: scheduling.values.concurrencyLimit,
        weight: scheduling.values.weight,
        groupIds: [...new Set(selectedGroupIds.value)],
      }
      const name = normalizeAccountName(customName.value)
      if (name !== normalizeAccountName(initialCustomName))
        payload.customName = name
      if (modelAccess.value && JSON.stringify(modelAccess.value) !== initialModelAccess)
        payload.modelAccess = { ...modelAccess.value, models: [...modelAccess.value.models] }
      if (editingAccount.value?.provider === 'openai')
        payload.turnStateInjectionEnabled = turnStateInjectionEnabled.value
      await updateAccount(payload)
      showEditModal.value = false
      await Promise.all([options.reloadAccounts(), options.reloadGroups()])
      toast.success('账号已更新')
    })
  }

  watch([showEditModal, saving], ([open, isSaving]) => {
    if (open || isSaving)
      return
    editingAccountId.value = null
    customName.value = ''
    initialCustomName = ''
    proxyMode.value = 'preserve'
    proxyId.value = ''
    schedulingEnabled.value = true
    turnStateInjectionEnabled.value = false
    concurrencyLimit.value = ''
    weight.value = '1'
    modelAccess.value = undefined
    initialModelAccess = ''
    selectedGroupIds.value = []
  })

  return {
    customName,
    showEditModal,
    editingAccount,
    schedulingEnabled,
    turnStateInjectionEnabled,
    concurrencyLimit,
    weight,
    modelAccess,
    proxyMode,
    proxyId,
    selectedGroupIds,
    saving,
    open,
    save,
  }
}
