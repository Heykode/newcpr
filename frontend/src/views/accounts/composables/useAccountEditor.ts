import type { Ref } from 'vue'
import type { AccountModelAccess, getAccounts } from '@/api'

import { computed, ref, shallowRef, watch } from 'vue'
import { updateAccount } from '@/api'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { normalizeAccountName } from '@/utils/account-name'
import { accountModelAccessError } from '../utils/modelAccess'
import { concurrencyLimitInput, parseAccountSchedulingForm, parseExcelModels } from '../utils/schedulingForm'

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
  const excelEnabled = shallowRef(false)
  let initialExcelEnabled = false
  const excelModels = shallowRef('gpt-5.6-sol, gpt-6-astra')
  const excelModelsFollowGlobal = shallowRef(true)
  const excelCacheCreationAsInput = shallowRef(false)
  const excelAutoDisableOn403 = shallowRef(false)
  let initialExcelCacheCreationAsInput = false
  let initialExcelAutoDisableOn403 = false
  let initialExcelModelsFollowGlobal = true
  let initialExcelModels = ''
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
    excelEnabled.value = account.responsesUpstream === 'excel'
    initialExcelEnabled = excelEnabled.value
    excelModels.value = (account.excelModels ?? ['gpt-5.6-sol', 'gpt-6-astra']).join(', ')
    excelModelsFollowGlobal.value = account.excelModelsFollowGlobal ?? false
    excelCacheCreationAsInput.value = account.excelCacheCreationAsInput ?? false
    excelAutoDisableOn403.value = account.excelAutoDisableOn403 ?? false
    initialExcelCacheCreationAsInput = excelCacheCreationAsInput.value
    initialExcelAutoDisableOn403 = excelAutoDisableOn403.value
    initialExcelModelsFollowGlobal = excelModelsFollowGlobal.value
    initialExcelModels = excelModels.value
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
    const excelAvailable = editingAccount.value?.provider === 'openai' && editingAccount.value.authenticationKind === 'oauth'
    const models = !excelAvailable || excelModelsFollowGlobal.value ? [] : parseExcelModels(excelModels.value)
    if (models === null) {
      toast.warning('Excel 模型最多 64 个，每个名称最多 128 个字母、数字、点、下划线或连字符')
      return
    }
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
      if (editingAccount.value?.provider === 'openai' && editingAccount.value.authenticationKind === 'oauth'
        && excelEnabled.value !== initialExcelEnabled) {
        payload.responsesUpstream = excelEnabled.value ? 'excel' : 'codex'
      }
      if (excelAvailable && (excelModelsFollowGlobal.value !== initialExcelModelsFollowGlobal
        || (!excelModelsFollowGlobal.value && excelModels.value !== initialExcelModels))) {
        payload.excelModelsFollowGlobal = excelModelsFollowGlobal.value
        if (!excelModelsFollowGlobal.value)
          payload.excelModels = models
      }
      if (excelAvailable && excelCacheCreationAsInput.value !== initialExcelCacheCreationAsInput)
        payload.excelCacheCreationAsInput = excelEnabled.value && excelCacheCreationAsInput.value
      if (excelAvailable && (excelAutoDisableOn403.value !== initialExcelAutoDisableOn403 || (initialExcelEnabled && !excelEnabled.value)))
        payload.excelAutoDisableOn403 = excelEnabled.value && excelAutoDisableOn403.value
      await updateAccount(payload)
      showEditModal.value = false
      toast.success('账号已更新')
      void Promise.all([options.reloadAccounts(), options.reloadGroups()]).catch(() => undefined)
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
    excelEnabled.value = false
    initialExcelEnabled = false
    excelModels.value = 'gpt-5.6-sol, gpt-6-astra'
    excelModelsFollowGlobal.value = true
    excelCacheCreationAsInput.value = false
    excelAutoDisableOn403.value = false
    initialExcelCacheCreationAsInput = false
    initialExcelAutoDisableOn403 = false
    initialExcelModelsFollowGlobal = true
    initialExcelModels = ''
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
    excelEnabled,
    excelModels,
    excelModelsFollowGlobal,
    excelCacheCreationAsInput,
    excelAutoDisableOn403,
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
