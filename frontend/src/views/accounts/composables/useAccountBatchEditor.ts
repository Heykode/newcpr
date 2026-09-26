import type { Ref } from 'vue'
import type { AccountModelAccess, getAccounts } from '@/api'
import type { RequestOptions } from '@/api/request'

import { computed, ref, shallowRef, watch } from 'vue'
import { batchUpdateAccounts } from '@/api'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { normalizeAccountName } from '@/utils/account-name'
import { accountModelAccessError } from '../utils/modelAccess'
import { concurrencyLimitInput, parseAccountSchedulingForm, parseExcelModels } from '../utils/schedulingForm'

type AccountRow = Awaited<ReturnType<typeof getAccounts>>['items'][number]

export function useAccountBatchEditor(options: {
  accounts: Ref<AccountRow[]>
  selectedIds: Ref<Set<string>>
  reloadAccounts: (options?: RequestOptions) => Promise<unknown>
  reloadGroups: () => Promise<unknown>
}) {
  const selectedAccountsById = new Map<string, AccountRow>()
  const showBatchEditModal = shallowRef(false)
  const customName = shallowRef('')
  const updateCustomName = ref(false)
  const excelAvailable = shallowRef(false)
  const schedulingEnabled = shallowRef(true)
  const excelEnabled = shallowRef(false)
  const excelModels = shallowRef('gpt-5.6-sol, gpt-6-astra')
  const excelModelsFollowGlobal = shallowRef(true)
  const excelCacheCreationAsInput = shallowRef(false)
  const excelAutoDisableOn403 = shallowRef(false)
  const updateExcelCacheCreationAsInput = ref(false)
  const updateExcelAutoDisableOn403 = ref(false)
  const updateExcelModels = ref(false)
  const concurrencyLimit = shallowRef('')
  const weight = shallowRef('1')
  const modelAccess = ref<AccountModelAccess | undefined>()
  const updateModelAccess = ref(false)
  const catalogAccountId = shallowRef<string>()
  const proxyMode = shallowRef('preserve')
  const proxyId = shallowRef('')
  const selectedGroupIds = ref<string[]>([])
  const updateEnabled = ref(false)
  const updateExcelEnabled = ref(false)
  const updateConcurrencyLimit = ref(false)
  const updateWeight = ref(false)
  const updateGroups = ref(false)
  const updateProxy = ref(false)
  const hasUpdates = computed(() =>
    updateCustomName.value
    || updateEnabled.value
    || (excelAvailable.value && updateExcelEnabled.value)
    || (excelAvailable.value && updateExcelModels.value)
    || (excelAvailable.value && updateExcelCacheCreationAsInput.value)
    || (excelAvailable.value && updateExcelAutoDisableOn403.value)
    || updateConcurrencyLimit.value
    || updateWeight.value
    || updateModelAccess.value
    || updateGroups.value
    || (updateProxy.value && proxyMode.value !== 'preserve'),
  )
  const saveAction = useAsyncAction()
  const saving = saveAction.loading

  function resetUpdateSelection() {
    updateCustomName.value = false
    updateEnabled.value = false
    updateExcelEnabled.value = false
    updateExcelModels.value = false
    updateExcelCacheCreationAsInput.value = false
    updateExcelAutoDisableOn403.value = false
    updateConcurrencyLimit.value = false
    updateWeight.value = false
    updateModelAccess.value = false
    updateGroups.value = false
    updateProxy.value = false
  }

  function open() {
    const accounts = selectedAccounts()
    if (accounts.length === 0)
      return
    const firstName = accounts[0]?.customName ?? ''
    customName.value = accounts.every(account => (account.customName ?? '') === firstName) ? firstName : ''

    schedulingEnabled.value = accounts.every(account => account.enabled)
    excelAvailable.value = accounts.every(account => account.provider === 'openai' && account.authenticationKind === 'oauth')
    excelEnabled.value = accounts.every(account => account.responsesUpstream === 'excel')
    excelModels.value = (accounts[0]?.excelModels ?? ['gpt-5.6-sol', 'gpt-6-astra']).join(', ')
    excelModelsFollowGlobal.value = accounts.every(account => account.excelModelsFollowGlobal ?? false)
    excelCacheCreationAsInput.value = accounts.every(account => account.excelCacheCreationAsInput ?? false)
    excelAutoDisableOn403.value = accounts.every(account => account.excelAutoDisableOn403 ?? false)
    proxyMode.value = 'preserve'
    proxyId.value = ''
    concurrencyLimit.value = sharedConcurrencyLimit(accounts)
    weight.value = sharedWeight(accounts)
    modelAccess.value = { mode: 'all', models: [] }
    catalogAccountId.value = accounts[0]?.id
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
    const modelError = updateModelAccess.value ? accountModelAccessError(modelAccess.value) : undefined
    if (modelError || (updateModelAccess.value && !modelAccess.value)) {
      toast.warning(modelError ?? '请选择模型限制模式')
      return
    }
    const scheduling = parseAccountSchedulingForm(
      updateConcurrencyLimit.value ? concurrencyLimit.value : '',
      updateWeight.value ? weight.value : '1',
    )
    const models = excelModelsFollowGlobal.value ? [] : parseExcelModels(excelModels.value)
    if (excelAvailable.value && updateExcelModels.value && models === null) {
      toast.warning('Excel 模型名称不合法，或超过 64 个')
      return
    }
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
      if (updateCustomName.value)
        payload.customName = normalizeAccountName(customName.value)
      if (updateEnabled.value)
        payload.enabled = schedulingEnabled.value
      if (excelAvailable.value && updateExcelEnabled.value)
        payload.responsesUpstream = excelEnabled.value ? 'excel' : 'codex'
      if (excelAvailable.value && updateExcelCacheCreationAsInput.value)
        payload.excelCacheCreationAsInput = excelCacheCreationAsInput.value
      if (excelAvailable.value && updateExcelAutoDisableOn403.value)
        payload.excelAutoDisableOn403 = excelAutoDisableOn403.value
      if (excelAvailable.value && updateExcelModels.value && models !== null) {
        payload.excelModelsFollowGlobal = excelModelsFollowGlobal.value
        if (!excelModelsFollowGlobal.value)
          payload.excelModels = models
      }
      if (updateConcurrencyLimit.value)
        payload.concurrencyLimit = scheduling.values.concurrencyLimit
      if (updateWeight.value)
        payload.weight = scheduling.values.weight
      if (updateModelAccess.value && modelAccess.value)
        payload.modelAccess = { ...modelAccess.value, models: [...modelAccess.value.models] }
      if (updateGroups.value)
        payload.groupIds = [...new Set(selectedGroupIds.value)]
      if (updateProxy.value && proxyMode.value !== 'preserve') {
        payload.outboundProxyId = proxyMode.value === 'direct' ? '' : proxyId.value.trim()
      }
      await batchUpdateAccounts(payload)
      showBatchEditModal.value = false
      options.selectedIds.value = new Set()
      toast.success(`已更新 ${accountIds.length} 个账号`)
      void Promise.all([options.reloadAccounts(), options.reloadGroups()]).catch(() => undefined)
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
    customName.value = ''
    schedulingEnabled.value = true
    excelAvailable.value = false
    excelEnabled.value = false
    proxyMode.value = 'preserve'
    proxyId.value = ''
    concurrencyLimit.value = ''
    weight.value = '1'
    modelAccess.value = undefined
    catalogAccountId.value = undefined
    selectedGroupIds.value = []
    resetUpdateSelection()
  })

  return {
    customName,
    updateCustomName,
    showBatchEditModal,
    excelAvailable,
    schedulingEnabled,
    excelEnabled,
    excelModels,
    excelModelsFollowGlobal,
    excelCacheCreationAsInput,
    excelAutoDisableOn403,
    updateExcelCacheCreationAsInput,
    updateExcelAutoDisableOn403,
    updateExcelModels,
    concurrencyLimit,
    weight,
    modelAccess,
    updateModelAccess,
    catalogAccountId,
    proxyMode,
    proxyId,
    selectedGroupIds,
    updateEnabled,
    updateExcelEnabled,
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
