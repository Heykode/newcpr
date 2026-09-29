import type { Ref } from 'vue'
import type { AccountModelAccess, getAccounts } from '@/api'
import type { AccountEgressDraft, AccountEgressReadState } from '@/utils/account-egress'
import type { Excel403Action } from '@/utils/excel-settings'
import { computed, onScopeDispose, ref, shallowRef, watch } from 'vue'

import { updateAccount } from '@/api'
import { getIpv6Egress } from '@/api/modules/ipv6-egress'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { accountEgressFromAccount, accountEgressPatch, sameAccountEgress } from '@/utils/account-egress'
import { normalizeAccountName } from '@/utils/account-name'
import { DEFAULT_EXCEL_MODELS, DEFAULT_EXCEL_MODELS_INPUT } from '@/utils/excel-defaults'
import { accountExcel403Action } from '@/utils/excel-settings'
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
  const excelModels = shallowRef(DEFAULT_EXCEL_MODELS_INPUT)
  const excelModelsFollowGlobal = shallowRef(true)
  const excelCacheCreationAsInput = shallowRef(false)
  const excelIgnoreEncryptedContent = shallowRef(false)
  const requestProxySource = shallowRef<import('@/utils/request-proxy-source').RequestProxySource>('account')
  const excel403Action = shallowRef<Excel403Action>('none')
  let initialExcelCacheCreationAsInput = false
  let initialExcelIgnoreEncryptedContent = false
  let initialExcel403Action: Excel403Action = 'none'
  let initialExcelModelsFollowGlobal = true
  let initialExcelModels = ''
  const concurrencyLimit = shallowRef('')
  const weight = shallowRef('1')
  const modelAccess = ref<AccountModelAccess | undefined>()
  let initialModelAccess = ''
  const proxyMode = shallowRef('')
  const proxyId = shallowRef('')
  const egressMode = shallowRef('fixed_ipv6_reuse')
  const egressReadState = shallowRef<AccountEgressReadState>({ loading: false, error: false })
  let initialEgress: AccountEgressDraft | undefined
  let egressController: AbortController | undefined
  const selectedGroupIds = ref<string[]>([])
  const saveAction = useAsyncAction()
  const saving = saveAction.loading
  const editingAccount = computed(() => {
    const accountId = editingAccountId.value
    return accountId
      ? options.accounts.value.find(account => account.id === accountId) ?? null
      : null
  })

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
    editingAccountId.value = account.id
    customName.value = account.customName ?? ''
    initialCustomName = customName.value
    schedulingEnabled.value = account.enabled
    excelEnabled.value = account.responsesUpstream === 'excel'
    initialExcelEnabled = excelEnabled.value
    excelModels.value = (account.excelModels ?? DEFAULT_EXCEL_MODELS).join(', ')
    excelModelsFollowGlobal.value = account.excelModelsFollowGlobal ?? false
    excelCacheCreationAsInput.value = account.excelCacheCreationAsInput ?? false
    excelIgnoreEncryptedContent.value = account.excelIgnoreEncryptedContent ?? false
    requestProxySource.value = account.requestProxySource ?? 'account'
    excel403Action.value = accountExcel403Action(account)
    initialExcelCacheCreationAsInput = excelCacheCreationAsInput.value
    initialExcelIgnoreEncryptedContent = excelIgnoreEncryptedContent.value
    initialExcel403Action = excel403Action.value
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
    const excelAvailable = editingAccount.value?.provider === 'openai' && editingAccount.value.authenticationKind === 'oauth'
    const models = !excelAvailable || excelModelsFollowGlobal.value ? [] : parseExcelModels(excelModels.value)
    if (models === null) {
      toast.warning('Excel 模型最多 64 个，每个名称最多 128 个字母、数字、点、下划线或连字符')
      return
    }
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
        ...(initialEgress && !sameAccountEgress(initialEgress, egress)
          ? accountEgressPatch(egress, editingAccount.value?.provider === 'openai')
          : {}),
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
        payload.excelCacheCreationAsInput = excelCacheCreationAsInput.value
      if (excelAvailable && (excelIgnoreEncryptedContent.value !== initialExcelIgnoreEncryptedContent || (initialExcelEnabled && !excelEnabled.value)))
        payload.excelIgnoreEncryptedContent = excelEnabled.value && excelIgnoreEncryptedContent.value
      if (excelAvailable && (excel403Action.value !== initialExcel403Action || (initialExcelEnabled && !excelEnabled.value)))
        payload.excel403Action = excelEnabled.value ? excel403Action.value : 'none'
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
    applyInitialEgress(undefined)
    egressReadState.value = { loading: false, error: false }
    schedulingEnabled.value = true
    excelEnabled.value = false
    initialExcelEnabled = false
    excelModels.value = DEFAULT_EXCEL_MODELS_INPUT
    excelModelsFollowGlobal.value = true
    excelCacheCreationAsInput.value = false
    excelIgnoreEncryptedContent.value = false
    requestProxySource.value = 'account'
    excel403Action.value = 'none'
    initialExcelCacheCreationAsInput = false
    initialExcelIgnoreEncryptedContent = false
    initialExcel403Action = 'none'
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
    excelIgnoreEncryptedContent,
    requestProxySource,
    excel403Action,
    concurrencyLimit,
    weight,
    modelAccess,
    proxyMode,
    proxyId,
    egressMode,
    egressReadState,
    selectedGroupIds,
    saving,
    open,
    save,
  }
}
