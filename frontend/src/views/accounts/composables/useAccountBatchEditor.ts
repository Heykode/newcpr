import type { Ref } from 'vue'
import type { getAccounts } from '@/api'
import type { RequestOptions } from '@/api/request'

import { computed, ref, shallowRef, watch } from 'vue'
import { batchUpdateAccounts } from '@/api'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { concurrencyLimitInput, parseAccountSchedulingForm } from '../utils/schedulingForm'

type AccountRow = Awaited<ReturnType<typeof getAccounts>>['items'][number]

export function useAccountBatchEditor(options: {
  accounts: Ref<AccountRow[]>
  selectedIds: Ref<Set<string>>
  reloadAccounts: (options?: RequestOptions) => Promise<unknown>
  reloadGroups: () => Promise<unknown>
}) {
  const selectedAccountsById = new Map<string, AccountRow>()
  const showBatchEditModal = shallowRef(false)
  const turnStateAvailable = shallowRef(false)
  const schedulingEnabled = shallowRef(true)
  const turnStateInjectionEnabled = shallowRef(false)
  const concurrencyLimit = shallowRef('')
  const weight = shallowRef('1')
  const proxyMode = shallowRef('preserve')
  const proxyId = shallowRef('')
  const selectedGroupIds = ref<string[]>([])
  const updateEnabled = ref(false)
  const updateTurnStateInjectionEnabled = ref(false)
  const updateConcurrencyLimit = ref(false)
  const updateWeight = ref(false)
  const updateGroups = ref(false)
  const updateProxy = ref(false)
  const hasUpdates = computed(() =>
    updateEnabled.value
    || (turnStateAvailable.value && updateTurnStateInjectionEnabled.value)
    || updateConcurrencyLimit.value
    || updateWeight.value
    || updateGroups.value
    || (updateProxy.value && proxyMode.value !== 'preserve'),
  )
  const saveAction = useAsyncAction()
  const saving = saveAction.loading

  function resetUpdateSelection() {
    updateEnabled.value = false
    updateTurnStateInjectionEnabled.value = false
    updateConcurrencyLimit.value = false
    updateWeight.value = false
    updateGroups.value = false
    updateProxy.value = false
  }

  function open() {
    const accounts = selectedAccounts()
    if (accounts.length === 0)
      return

    schedulingEnabled.value = accounts.every(account => account.enabled)
    turnStateAvailable.value = accounts.every(account => account.provider === 'openai')
    turnStateInjectionEnabled.value = accounts.every(account => account.turnStateInjectionEnabled)
    proxyMode.value = 'preserve'
    proxyId.value = ''
    concurrencyLimit.value = sharedConcurrencyLimit(accounts)
    weight.value = sharedWeight(accounts)
    selectedGroupIds.value = sharedGroupIds(accounts)
    resetUpdateSelection()
    showBatchEditModal.value = true
  }

  async function save() {
    if (saving.value || options.selectedIds.value.size === 0)
      return
    if (!hasUpdates.value) {
      toast.warning('请先勾选要更新的设置')
      return
    }
    const scheduling = parseAccountSchedulingForm(
      updateConcurrencyLimit.value ? concurrencyLimit.value : '',
      updateWeight.value ? weight.value : '1',
    )
    if (updateProxy.value && proxyMode.value === 'proxy' && !proxyId.value.trim()) {
      toast.warning('请选择已通过测试的代理')
      return
    }
    if (!scheduling.valid) {
      toast.warning(scheduling.message)
      return
    }

    await saveAction.run(async () => {
      const accountIds = selectedAccounts().map(account => account.id)
      const payload: Parameters<typeof batchUpdateAccounts>[0] = {
        accountIds,
      }
      if (updateEnabled.value)
        payload.enabled = schedulingEnabled.value
      if (turnStateAvailable.value && updateTurnStateInjectionEnabled.value)
        payload.turnStateInjectionEnabled = turnStateInjectionEnabled.value
      if (updateConcurrencyLimit.value)
        payload.concurrencyLimit = scheduling.values.concurrencyLimit
      if (updateWeight.value)
        payload.weight = scheduling.values.weight
      if (updateGroups.value)
        payload.groupIds = [...new Set(selectedGroupIds.value)]
      if (updateProxy.value && proxyMode.value !== 'preserve') {
        payload.outboundProxyId = proxyMode.value === 'direct' ? '' : proxyId.value.trim()
      }
      await batchUpdateAccounts(payload)
      showBatchEditModal.value = false
      options.selectedIds.value = new Set()
      await Promise.all([options.reloadAccounts(), options.reloadGroups()])
      toast.success(`已更新 ${accountIds.length} 个账号`)
    }, { onError: () => void options.reloadAccounts({ silent: true }) })
  }

  function selectedAccounts() {
    return [...options.selectedIds.value].map((accountId) => {
      const account = selectedAccountsById.get(accountId)
      if (!account)
        throw new Error(`账号 ${accountId} 的页面数据已失效，请重新选择`)
      return account
    })
  }

  watch(
    [options.accounts, options.selectedIds],
    ([accounts, selectedIds]) => {
      for (const account of accounts) {
        if (selectedIds.has(account.id))
          selectedAccountsById.set(account.id, account)
      }
      for (const accountId of selectedAccountsById.keys()) {
        if (!selectedIds.has(accountId))
          selectedAccountsById.delete(accountId)
      }
    },
    { immediate: true, flush: 'sync' },
  )

  watch([showBatchEditModal, saving], ([open, isSaving]) => {
    if (open || isSaving)
      return
    schedulingEnabled.value = true
    turnStateAvailable.value = false
    turnStateInjectionEnabled.value = false
    proxyMode.value = 'preserve'
    proxyId.value = ''
    concurrencyLimit.value = ''
    weight.value = '1'
    selectedGroupIds.value = []
    resetUpdateSelection()
  })

  return {
    showBatchEditModal,
    turnStateAvailable,
    schedulingEnabled,
    turnStateInjectionEnabled,
    concurrencyLimit,
    weight,
    proxyMode,
    proxyId,
    selectedGroupIds,
    updateEnabled,
    updateTurnStateInjectionEnabled,
    updateConcurrencyLimit,
    updateWeight,
    updateGroups,
    updateProxy,
    hasUpdates,
    saving,
    open,
    save,
  }
}

function sharedConcurrencyLimit(accounts: AccountRow[]) {
  const first = accounts[0]?.concurrencyLimit ?? null
  return accounts.every(account => account.concurrencyLimit === first)
    ? concurrencyLimitInput(first)
    : ''
}

function sharedWeight(accounts: AccountRow[]) {
  const first = accounts[0]?.weight ?? 1
  return accounts.every(account => account.weight === first) ? String(first) : '1'
}

function sharedGroupIds(accounts: AccountRow[]) {
  const [first, ...rest] = accounts
  if (!first)
    return []

  const shared = new Set(first.groups.map(group => group.id))
  for (const account of rest) {
    const current = new Set(account.groups.map(group => group.id))
    for (const groupId of shared) {
      if (!current.has(groupId))
        shared.delete(groupId)
    }
  }
  return [...shared]
}
