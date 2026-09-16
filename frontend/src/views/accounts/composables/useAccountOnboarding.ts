import type { AccountImportTask, getAccounts } from '@/api'

import { computed, onScopeDispose, ref, shallowRef, watch } from 'vue'
import {
  completeAccountOAuth,
  createAccountImportTask,
  startAccountOAuth,
} from '@/api'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { isRecord } from '@/utils/object'
import { formatProviderLabel, isSupportedProvider } from '@/utils/providers'
import { accountImportSettings, accountProxyError, emptyAccountCreateForm } from '../components/AccountCreateModal/model'

type AccountRow = Awaited<ReturnType<typeof getAccounts>>['items'][number]
type ImportProvider = 'openai' | 'xai'

interface MixedImportDocument {
  provider: ImportProvider
  document: Record<string, unknown>
}

type OpenAiTokenImportMode = 'access_token' | 'refresh_token'

const MAX_TOKEN_IMPORT_COUNT = 200

export function useAccountOnboarding(options: {
  reload: () => Promise<unknown>
  onImportTaskCreated: (task: AccountImportTask) => void
}) {
  const createModalOpen = shallowRef(false)
  const reauthorizingAccount = shallowRef<AccountRow | null>(null)
  const creatingAccountAction = useAsyncAction()
  const authorizingOAuthAction = useAsyncAction()
  const creatingAccount = creatingAccountAction.loading
  const authorizingOAuth = authorizingOAuthAction.loading
  const createForm = ref(emptyAccountCreateForm())
  let submissionId: string | undefined
  // Modal-local credential material; never persist or log this comparison key.
  let submissionKey: string | undefined

  function clearSubmission() {
    submissionId = undefined
    submissionKey = undefined
  }

  onScopeDispose(clearSubmission)

  const showCreateModal = computed({
    get: () => createModalOpen.value,
    set: (value: boolean) => {
      createModalOpen.value = value
      if (!value) {
        clearSubmission()
        reauthorizingAccount.value = null
        createForm.value = emptyAccountCreateForm()
      }
    },
  })

  async function handleCreate() {
    if (createForm.value.mode === 'oauth') {
      await completeOAuth()
      return
    }
    if (creatingAccount.value)
      return

    await creatingAccountAction.run(
      async () => {
        const proxyError = accountProxyError(createForm.value)
        if (proxyError)
          throw new Error(proxyError)
        const mode = createForm.value.mode
        if (mode === 'oauth')
          throw new Error('请选择凭据导入方式')
        const documents = createForm.value.provider === 'batch'
          ? parseMixedImportDocuments(parseImportJson(createForm.value.importTexts.json))
          : accountImportDocuments(requireImportProvider(createForm.value.provider), mode, createForm.value.importTexts[mode])
        if (documents.length > MAX_TOKEN_IMPORT_COUNT)
          throw new Error(`单次最多导入 ${MAX_TOKEN_IMPORT_COUNT} 个条目`)
        const settings = accountImportSettings(createForm.value)
        const items = documents.map(entry => ({
          provider: entry.provider,
          data: entry.document,
          settings,
          outboundProxyId: createForm.value.proxyMode === 'proxy' ? createForm.value.proxyId.trim() : undefined,
        }))
        const encoded = JSON.stringify(items, (_key, value: unknown) => isRecord(value)
          ? Object.fromEntries(Object.keys(value).sort().map(key => [key, value[key]]))
          : value)
        if (!submissionId || submissionKey !== encoded) {
          submissionId = generateSubmissionId()
          submissionKey = encoded
        }
        const task = await createAccountImportTask({
          submissionId,
          // Canonical key order also keeps the server fingerprint stable.
          items: JSON.parse(encoded),
        })
        showCreateModal.value = false
        options.onImportTaskCreated(task)
        toast.success('导入任务已创建')
      },
    )
  }

  async function handleAuthorizeOAuth() {
    if (authorizingOAuth.value)
      return

    await authorizingOAuthAction.run(
      async () => {
        const input = newAccountInput()
        const account = reauthorizingAccount.value
        const proxyError = accountProxyError(createForm.value)
        if (!account && proxyError)
          throw new Error(proxyError)
        const result = await startAccountOAuth({
          ...input,
          outboundProxyId: !account && createForm.value.proxyMode === 'proxy' ? createForm.value.proxyId.trim() : undefined,
          ...(account
            ? {
                accountId: account.id,
              }
            : {}),
        })

        createForm.value = {
          ...createForm.value,
          oauthFlowId: result.flowId,
          oauthAuthUrl: result.authorizationUrl,
          oauthCallback: '',
        }
        toast.success('授权链接已生成')
      },
    )
  }

  async function completeOAuth() {
    if (creatingAccount.value)
      return

    await creatingAccountAction.run(
      async () => {
        if (!createForm.value.oauthFlowId)
          throw new Error('请先生成授权链接')

        const callbackUrl = createForm.value.oauthCallback.trim()
        if (!callbackUrl) {
          throw new Error(createForm.value.provider === 'xai'
            ? '请粘贴 OAuth 回调地址、含 code 和 state 的查询字符串或授权码'
            : '请粘贴 OAuth 回调地址')
        }
        await completeAccountOAuth({
          provider: createForm.value.provider,
          flowId: createForm.value.oauthFlowId,
          callbackUrl,
          settings: reauthorizingAccount.value ? undefined : accountImportSettings(createForm.value),
        })
        await finishCreate(
          reauthorizingAccount.value
            ? '账号重新授权成功'
            : createForm.value.provider === 'xai'
              ? 'xAI OAuth 账号已添加'
              : 'OpenAI OAuth 账号已添加',
        )
      },
    )
  }

  function openCreateAccount() {
    clearSubmission()
    reauthorizingAccount.value = null
    createForm.value = emptyAccountCreateForm()
    showCreateModal.value = true
  }

  function openReauthorizeAccount(account: AccountRow) {
    if (account.provider !== 'openai' && account.provider !== 'xai')
      return
    clearSubmission()
    reauthorizingAccount.value = account
    createForm.value = {
      ...emptyAccountCreateForm(),
      provider: account.provider,
      step: 'import',
      mode: 'oauth',
    }
    showCreateModal.value = true
    void handleAuthorizeOAuth()
  }

  function newAccountInput() {
    const account = reauthorizingAccount.value
    return {
      provider: createForm.value.provider,
      name: account?.name || account?.email || `${createForm.value.provider} OAuth`,
    }
  }

  async function finishCreate(message: string) {
    showCreateModal.value = false
    await options.reload()
    toast.success(message)
  }

  watch(
    () => createForm.value.provider,
    () => {
      createForm.value = {
        ...createForm.value,
        mode: createForm.value.provider === 'batch' ? 'json' : 'oauth',
        importTexts: { access_token: '', refresh_token: '', json: '' },
        oauthFlowId: '',
        oauthAuthUrl: '',
        oauthCallback: '',
      }
    },
    { flush: 'sync' },
  )

  watch(
    [
      () => createForm.value.proxyMode,
      () => createForm.value.proxyMode === 'proxy' ? createForm.value.proxyId.trim() : '',
    ],
    () => {
      createForm.value.oauthFlowId = ''
      createForm.value.oauthAuthUrl = ''
      createForm.value.oauthCallback = ''
    },
    { flush: 'sync' },
  )

  return {
    showCreateModal,
    reauthorizingAccount,
    creatingAccount,
    authorizingOAuth,
    createForm,
    handleCreate,
    handleAuthorizeOAuth,
    openCreateAccount,
    openReauthorizeAccount,
  }
}

function parseImportJson(value: string) {
  try {
    return JSON.parse(value)
  }
  catch {
    throw new Error('JSON 格式不正确')
  }
}

function requireImportProvider(value: string): ImportProvider {
  if (isSupportedProvider(value))
    return value
  throw new Error('请选择要导入的账号平台')
}

function accountImportDocuments(
  provider: ImportProvider,
  mode: string,
  value: string,
): MixedImportDocument[] {
  if (provider === 'openai' && isOpenAiTokenImportMode(mode)) {
    return parseOpenAiTokenImport(value, mode).map(document => ({ provider, document }))
  }
  return providerImportDocuments(parseImportJson(value), provider)
}

function parseOpenAiTokenImport(value: string, mode: OpenAiTokenImportMode) {
  const tokens = value
    .split(/\r?\n/)
    .map(token => token.trim())
    .filter(Boolean)
  const label = mode === 'access_token' ? 'Access Token' : 'Refresh Token'

  if (tokens.length === 0)
    throw new Error(`请至少粘贴一个 ${label}`)
  if (tokens.length > MAX_TOKEN_IMPORT_COUNT)
    throw new Error(`单次最多导入 ${MAX_TOKEN_IMPORT_COUNT} 个 ${label}`)

  const credentialKey = mode === 'access_token' ? 'accessToken' : 'refreshToken'
  return tokens.map(token => ({ accounts: [{ [credentialKey]: token }] }))
}

function generateSubmissionId() {
  // 与额度重置一致，兼容普通 HTTP 管理端没有 randomUUID 的情况。
  const bytes = globalThis.crypto.getRandomValues(new Uint8Array(16))
  bytes[6] = (bytes[6] & 0x0F) | 0x40
  bytes[8] = (bytes[8] & 0x3F) | 0x80
  const hex = Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join('')
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
}

function isOpenAiTokenImportMode(value: string): value is OpenAiTokenImportMode {
  return value === 'access_token' || value === 'refresh_token'
}

function providerImportDocuments(value: unknown, provider: ImportProvider): MixedImportDocument[] {
  if (isRecord(value) && Array.isArray(value.documents)) {
    const documents = parseMixedImportDocuments(value)
      .filter(entry => entry.provider === provider)
    if (documents.length === 0) {
      const label = formatProviderLabel(provider)
      throw new Error(`批量导入文件不包含 ${label} 账号文档`)
    }
    return documents
  }
  if (!isRecord(value))
    throw new Error('导入文件必须是 JSON object')
  return [{ provider, document: value }]
}

function parseMixedImportDocuments(value: unknown): MixedImportDocument[] {
  if (!isRecord(value) || !Array.isArray(value.documents))
    throw new Error('批量导入文件必须是 CPR 多平台导出文件')

  const documents: MixedImportDocument[] = []
  for (const entry of value.documents) {
    if (!isRecord(entry))
      throw new Error('批量导入文件包含无效的 Provider 文档')
    const provider = entry.provider
    if (!isSupportedProvider(provider))
      throw new Error('批量导入文件包含无效的 Provider 文档')
    if (!isRecord(entry.document))
      throw new Error('批量导入文件包含无效的 Provider 文档')
    documents.push({ provider, document: entry.document })
  }

  if (documents.length === 0)
    throw new Error('批量文件没有可导入的账号文档')
  return documents
}
