import type { Ref, ShallowRef } from 'vue'
import type { getApiKeys } from '@/api'
import { computed, onScopeDispose, shallowRef } from 'vue'

import { API_BASE_URL } from '@/api/constants'
import { buildCodexCcSwitchImportDeeplink } from '../utils/ccswitchImport'

// “使用密钥”弹窗展示明文时，在列表行上补挂 reveal 得到的完整 key。
type ApiKeyRow = Awaited<ReturnType<typeof getApiKeys>>['items'][number] & { key?: string }

// 密钥使用与 CCSwitch 导入编排：服务根地址推导、deeplink 跳转、
// “使用密钥”弹窗的明文补全与打开。
export function useApiKeyUse(options: {
  createdKey: Readonly<Ref<string>>
  createdKeyName: Readonly<Ref<string>>
  revealPlaintextKey: (apiKey: ApiKeyRow, action?: 'copy' | 'configuration' | 'import') => Promise<string | undefined>
}) {
  const showUseKeyModal = shallowRef(false)
  const selectedUseKey: ShallowRef<ApiKeyRow | null> = shallowRef(null)
  const showImportModal = shallowRef(false)
  let pendingImport: { apiKey: string, baseUrl: string, providerName: string } | undefined
  let disposed = false
  let requestGeneration = 0

  const serviceRootUrl = computed(() => resolveServiceRootUrl())
  const openAiBaseUrl = computed(() => `${serviceRootUrl.value}/v1`)

  function resolveServiceRootUrl() {
    const normalizedApiBase = API_BASE_URL.trim().replace(/\/+$/, '')

    if (/^https?:\/\//i.test(normalizedApiBase)) {
      return normalizedApiBase
    }

    if (typeof window === 'undefined') {
      return normalizedApiBase
    }

    const origin = window.location.origin.replace(/\/+$/, '')
    if (!normalizedApiBase) {
      return origin
    }

    return `${origin}${normalizedApiBase.startsWith('/') ? normalizedApiBase : `/${normalizedApiBase}`}`
  }

  function importCreatedKeyToCcs() {
    if (!options.createdKey.value)
      return

    pendingImport = {
      apiKey: options.createdKey.value,
      baseUrl: openAiBaseUrl.value,
      providerName: options.createdKeyName.value || 'codex-proxy-rs',
    }
    showImportModal.value = true
  }

  async function openUseKeyModal(apiKey: ApiKeyRow) {
    const current = ++requestGeneration
    const key = await options.revealPlaintextKey(apiKey)
    if (!key || disposed || current !== requestGeneration)
      return
    selectedUseKey.value = { ...apiKey, key }
    showUseKeyModal.value = true
  }

  async function importToCcs(apiKey: ApiKeyRow) {
    const current = ++requestGeneration
    const key = await options.revealPlaintextKey(apiKey, 'import')
    if (!key || disposed || current !== requestGeneration)
      return
    pendingImport = {
      apiKey: key,
      baseUrl: openAiBaseUrl.value,
      providerName: apiKey.name || apiKey.prefix || 'codex-proxy-rs',
    }
    showImportModal.value = true
  }

  function confirmImport() {
    if (!disposed && pendingImport)
      window.location.href = buildCodexCcSwitchImportDeeplink(pendingImport)
    pendingImport = undefined
  }

  function clearImport() {
    if (!showImportModal.value)
      pendingImport = undefined
  }

  function clearUseKey() {
    if (!showUseKeyModal.value) {
      requestGeneration += 1
      selectedUseKey.value = null
    }
  }

  onScopeDispose(() => {
    disposed = true
    requestGeneration += 1
    selectedUseKey.value = null
    pendingImport = undefined
  })

  return {
    showImportModal,
    confirmImport,
    clearImport,
    showUseKeyModal,
    clearUseKey,
    selectedUseKey,
    serviceRootUrl,
    openAiBaseUrl,
    importCreatedKeyToCcs,
    openUseKeyModal,
    importToCcs,
  }
}
