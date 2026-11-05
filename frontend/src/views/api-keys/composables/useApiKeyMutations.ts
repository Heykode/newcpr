import type { Ref } from 'vue'
import type { ApiKeyBudgetPeriod, getApiKeys } from '@/api'
import { cloneDeep } from 'es-toolkit'
import { onScopeDispose, ref, shallowRef, watch } from 'vue'
import {
  createApiKey,
  deleteApiKey,
  disableApiKey,
  enableApiKey,
  resetApiKeyBudget,
  revealApiKey,
  updateApiKey,
} from '@/api'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { useCopyText } from '@/composables/useCopyText'
import { useIdSet } from '@/composables/useIdSet'
import { apiKeyFormErrors } from '../utils/formValidation'

type ApiKeyRow = Awaited<ReturnType<typeof getApiKeys>>['items'][number]

export interface ApiKeyFormValue {
  installationId: string
  providerRequestProfileOverrides: Record<string, Record<string, unknown>>
  name: string
  label: string
  groupIds: string[]
  maxConcurrency: string
  requestsPerMinute: string
  dailyLimitUsd: string
  weeklyLimitUsd: string
}

export function useApiKeyMutations(options: {
  selectedIds: Ref<Set<string>>
  reload: () => Promise<unknown>
}) {
  const copyText = useCopyText()
  const showFormModal = shallowRef(false)
  const showDeleteModal = shallowRef(false)
  const showSingleDeleteModal = shallowRef(false)
  const showKeyModal = shallowRef(false)
  const showAllAccountsConfirm = shallowRef(false)
  const createdKey = shallowRef('')
  const createdKeyName = shallowRef('')
  const editingKey = shallowRef<ApiKeyRow | null>(null)
  const pendingDeleteKey = shallowRef<ApiKeyRow | null>(null)
  const savingKeyAction = useAsyncAction()
  const deletingKeyAction = useAsyncAction()
  const batchDeletingAction = useAsyncAction()
  const updatingStatusKeys = useIdSet<string>()
  const revealingKeys = useIdSet<string>()
  const copyingKeys = useIdSet<string>()
  const configurationKeys = useIdSet<string>()
  const importingKeys = useIdSet<string>()
  const savingKey = savingKeyAction.loading
  const deletingKey = deletingKeyAction.loading
  const batchDeleting = batchDeletingAction.loading
  const deletingCount = shallowRef(0)
  watch([showDeleteModal, batchDeleting, () => options.selectedIds.value.size], ([open, busy, count]) => {
    if (open && !busy)
      deletingCount.value = count
  }, { flush: 'sync' })
  const updatingStatusKeyIds = updatingStatusKeys.ids
  const revealingKeyIds = revealingKeys.ids
  const form = ref<ApiKeyFormValue>(emptyForm())
  const showBudgetResetModal = shallowRef(false)
  const pendingBudgetKey = shallowRef<ApiKeyRow | null>(null)
  const budgetPeriod = shallowRef<ApiKeyBudgetPeriod>('all')
  const resetBudgetAction = useAsyncAction()
  const resettingBudget = resetBudgetAction.loading

  function requestBudgetReset(key: ApiKeyRow) {
    if (resettingBudget.value)
      return
    pendingBudgetKey.value = key
    budgetPeriod.value = 'all'
    showBudgetResetModal.value = true
  }

  async function handleBudgetReset() {
    const key = pendingBudgetKey.value
    if (!key || resettingBudget.value)
      return
    await resetBudgetAction.run(async () => {
      await resetApiKeyBudget({ id: key.id, period: budgetPeriod.value })
      showBudgetResetModal.value = false
      toast.success('已用额度已重置')
      await options.reload()
    }, { onError: () => void options.reload() })
  }

  function openCreate() {
    editingKey.value = null
    form.value = emptyForm()
    showFormModal.value = true
  }

  function openEdit(key: ApiKeyRow) {
    editingKey.value = key
    const profiles = cloneDeep(key.providerRequestProfileOverrides ?? {})
    const openai = profiles.openai
    const identity = (openai?.inherit as Record<string, unknown> | undefined) ?? openai
    const installationId = typeof identity?.installationId === 'string' ? identity.installationId : ''
    if (openai?.inherit)
      delete profiles.openai
    else if (openai)
      delete openai.installationId
    form.value = {
      installationId,
      providerRequestProfileOverrides: profiles,
      name: key.name,
      label: key.label ?? '',
      groupIds: key.groups.map(group => group.id),
      maxConcurrency: limitInputValue(key.maxConcurrency),
      requestsPerMinute: limitInputValue(key.requestsPerMinute),
      dailyLimitUsd: limitInputValue(key.dailyLimitUsd),
      weeklyLimitUsd: limitInputValue(key.weeklyLimitUsd),
    }
    showFormModal.value = true
  }

  function requestSave() {
    if (!validateForm() || savingKey.value)
      return
    if (form.value.groupIds.length === 0) {
      showAllAccountsConfirm.value = true
      return
    }
    void save()
  }

  async function confirmAllAccountsScope() {
    showAllAccountsConfirm.value = false
    await save()
  }

  async function save() {
    if (!validateForm() || savingKey.value)
      return

    await savingKeyAction.run(
      async () => {
        const payload = {
          name: form.value.name.trim(),
          label: form.value.label.trim() || null,
          groupIds: [...new Set(form.value.groupIds)],
          maxConcurrency: parseLimit(form.value.maxConcurrency),
          requestsPerMinute: parseLimit(form.value.requestsPerMinute),
          dailyLimitUsd: form.value.dailyLimitUsd.trim() || '0',
          weeklyLimitUsd: form.value.weeklyLimitUsd.trim() || '0',
        }
        const profiles: Record<string, Record<string, unknown> | null> = cloneDeep(form.value.providerRequestProfileOverrides)
        const installationId = form.value.installationId
        if (installationId)
          profiles.openai = profiles.openai ? { ...profiles.openai, installationId } : { inherit: { installationId } }
        const current = editingKey.value
        if (current) {
          for (const provider of Object.keys(current.providerRequestProfileOverrides ?? {})) {
            if (!Object.hasOwn(profiles, provider))
              profiles[provider] = null
          }
          await updateApiKey({ id: current.id, ...payload, providerRequestProfileOverrides: profiles })
        }
        else {
          const result = await createApiKey({ ...payload, providerRequestProfileOverrides: profiles })
          createdKey.value = result.plaintextKey
          createdKeyName.value = payload.name
        }

        showFormModal.value = false
        await options.reload()
        if (current) {
          toast.success('API Key 已更新')
        }
        else {
          showKeyModal.value = true
        }
      },
      { onError: () => void options.reload() },
    )
  }

  function validateForm() {
    const firstError = Object.values(apiKeyFormErrors(form.value))[0]
    if (firstError)
      toast.warning(firstError)
    return !firstError
  }

  function requestDeleteKey(key: ApiKeyRow) {
    pendingDeleteKey.value = key
    showSingleDeleteModal.value = true
  }

  async function handleDelete() {
    if (deletingKey.value)
      return
    const keyId = pendingDeleteKey.value?.id
    if (!keyId)
      return

    await deletingKeyAction.run(
      async () => {
        await deleteApiKey({ id: keyId })
        const remaining = new Set(options.selectedIds.value)
        remaining.delete(keyId)
        options.selectedIds.value = remaining
        showSingleDeleteModal.value = false
        await options.reload()
        toast.success('删除成功')
      },
      { onError: () => void options.reload() },
    )
  }

  async function handleBatchDelete() {
    if (batchDeleting.value || options.selectedIds.value.size === 0)
      return

    await batchDeletingAction.run(
      async () => {
        const deleteCount = options.selectedIds.value.size
        for (const keyId of [...options.selectedIds.value]) {
          await deleteApiKey({ id: keyId })
          const remaining = new Set(options.selectedIds.value)
          remaining.delete(keyId)
          options.selectedIds.value = remaining
        }
        showDeleteModal.value = false
        await options.reload()
        toast.success(`已删除 ${deleteCount} 个 API Key`)
      },
      { onError: () => void options.reload() },
    )
  }

  async function handleToggleStatus(key: ApiKeyRow) {
    await updatingStatusKeys.run(key.id, async () => {
      try {
        const mutation = key.enabled ? disableApiKey : enableApiKey
        await mutation({ id: key.id })
        await options.reload()
        toast.success(key.enabled ? '已禁用' : '已启用')
      }
      catch {
        void options.reload()
      }
    })
  }

  async function copyToClipboard(text: string) {
    await copyText(text, { notifyWhenEmpty: true })
  }

  async function revealPlaintextKey(apiKey: ApiKeyRow, action: 'copy' | 'configuration' | 'import' = 'configuration') {
    try {
      const actions = action === 'copy' ? copyingKeys : action === 'import' ? importingKeys : configurationKeys
      const result = await revealingKeys.run(apiKey.id, () => actions.run(apiKey.id, () => revealApiKey({ id: apiKey.id })))
      if (!result)
        return undefined
      if (!result.plaintextKey) {
        toast.error('完整 API Key 不可用')
        return undefined
      }
      return result.plaintextKey
    }
    catch {
      return undefined
    }
  }

  async function copyApiKey(apiKey: ApiKeyRow) {
    const key = await revealPlaintextKey(apiKey, 'copy')
    if (key)
      await copyToClipboard(key)
  }

  function clearCreatedKey() {
    if (!showKeyModal.value) {
      createdKey.value = ''
      createdKeyName.value = ''
    }
  }
  function clearForm() {
    if (!showFormModal.value) {
      editingKey.value = null
      form.value = emptyForm()
    }
  }

  onScopeDispose(() => {
    createdKey.value = ''
    createdKeyName.value = ''
  })

  return {
    deletingCount,
    showBudgetResetModal,
    pendingBudgetKey,
    budgetPeriod,
    resettingBudget,
    requestBudgetReset,
    handleBudgetReset,
    showFormModal,
    clearForm,
    clearCreatedKey,
    showDeleteModal,
    showSingleDeleteModal,
    showKeyModal,
    showAllAccountsConfirm,
    createdKey,
    createdKeyName,
    editingKey,
    pendingDeleteKey,
    savingKey,
    deletingKey,
    batchDeleting,
    updatingStatusKeyIds,
    revealingKeyIds,
    copyingKeyIds: copyingKeys.ids,
    configurationKeyIds: configurationKeys.ids,
    importingKeyIds: importingKeys.ids,
    form,
    openCreate,
    openEdit,
    requestSave,
    confirmAllAccountsScope,
    requestDeleteKey,
    handleDelete,
    handleBatchDelete,
    handleToggleStatus,
    copyToClipboard,
    revealPlaintextKey,
    copyApiKey,
  }
}

function emptyForm(): ApiKeyFormValue {
  return {
    installationId: '',
    providerRequestProfileOverrides: {},
    name: '',
    label: '',
    groupIds: [],
    maxConcurrency: '',
    requestsPerMinute: '',
    dailyLimitUsd: '',
    weeklyLimitUsd: '',
  }
}

function limitInputValue(limit: string | number) {
  return Number(limit) === 0 ? '' : String(limit)
}

function parseLimit(value: string) {
  return Number(value)
}
