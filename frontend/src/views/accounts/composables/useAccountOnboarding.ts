import type { AccountImportTask, getAccounts } from '@/api'
import type { CredentialTarget } from '@/api/modules/relogin'

import { computed, onScopeDispose, ref, shallowRef, watch } from 'vue'
import {
  completeAccountOAuth,
  completeExcelOAuth,
  createAccountImportTask,
  importExcelCredential,
  startAccountOAuth,
  startExcelOAuth,
} from '@/api'
import { applyQualityTemplate } from '@/api/modules/quality-ops'
import { enrollRelogin } from '@/api/modules/relogin'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { errorMessage } from '@/utils/async'
import { isRecord } from '@/utils/object'
import { formatProviderLabel, isSupportedProvider } from '@/utils/providers'
import { accountCreateEgress, accountImportQualityTemplate, accountImportSettings, accountImportTemplate, accountProxyError, emptyAccountCreateForm } from '../components/AccountCreateModal/model'
import { accountImportBatches, splitAccountDocuments } from './account-import-batches'

type AccountRow = Awaited<ReturnType<typeof getAccounts>>['items'][number]
type ImportProvider = 'openai' | 'xai'

interface MixedImportDocument {
  provider: ImportProvider
  credentialTarget?: 'codex' | 'excel'
  document: Record<string, unknown>
}

type OpenAiTokenImportMode = 'access_token' | 'refresh_token'

const MAX_TOKEN_IMPORT_COUNT = 200

export function useAccountOnboarding(options: {
  reload: () => Promise<unknown>
  onImportTaskCreated: (task: AccountImportTask) => void
}) {
  const createModalOpen = shallowRef(false)
  const enrollments = shallowRef<{ id: string, credentialTarget: CredentialTarget }[]>([])
  const reauthorizingAccount = shallowRef<AccountRow | null>(null)
  const creatingAccountAction = useAsyncAction()
  const authorizingOAuthAction = useAsyncAction()
  const creatingAccount = creatingAccountAction.loading
  const authorizingOAuth = authorizingOAuthAction.loading
  const createForm = ref(emptyAccountCreateForm())
  let submissionIds: string[] = []
  let submittedBatches = 0
  let importedExcelEntries = 0
  let oauthExpiryTimer: ReturnType<typeof setTimeout> | undefined
  function clearOAuthExpiry() {
    clearTimeout(oauthExpiryTimer)
    oauthExpiryTimer = undefined
  }
  onScopeDispose(clearOAuthExpiry)
  watch([createModalOpen, () => createForm.value.oauthFlowId], clearOAuthExpiry, { flush: 'sync' })
  // Modal-local credential material; never persist or log this comparison key.
  let submissionKey: string | undefined

  function clearSubmission() {
    submissionIds = []
    submittedBatches = 0
    importedExcelEntries = 0
    submissionKey = undefined
  }

  onScopeDispose(clearSubmission)

  const showCreateModal = computed({
    get: () => createModalOpen.value,
    set: (value: boolean) => {
      createModalOpen.value = value
      if (!value)
        clearSubmission()
    },
  })

  function clearCreate() {
    if (showCreateModal.value)
      return
    clearSubmission()
    reauthorizingAccount.value = null
    createForm.value = emptyAccountCreateForm()
  }

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
        if (mode === 'two_fa') {
          if (createForm.value.provider !== 'openai' || reauthorizingAccount.value)
            throw new Error('2FA 导入仅支持新增 OpenAI 账号')
          const credentialTarget = createForm.value.credentialTarget
          const result = await enrollRelogin({
            text: createForm.value.importTexts.two_fa,
            replaceExisting: createForm.value.replaceExisting2fa,
            settings: accountImportSettings(createForm.value, 'openai'),
            outboundProxyId: createForm.value.proxyMode === 'proxy' ? createForm.value.proxyId.trim() : undefined,
            qualityTemplate: accountImportQualityTemplate(createForm.value),
            credentialTarget,
          })
          enrollments.value = [
            ...enrollments.value.filter(entry => entry.credentialTarget !== credentialTarget || !result.ids.includes(entry.id)),
            ...[...new Set(result.ids)].map(id => ({ id, credentialTarget })),
          ]
          showCreateModal.value = false
          toast.success('2FA 登录入池任务已创建')
          return
        }
        const documents = createForm.value.provider === 'batch'
          ? parseMixedImportDocuments(parseImportJson(createForm.value.importTexts.json))
          : accountImportDocuments(requireImportProvider(createForm.value.provider), mode, createForm.value.importTexts[mode])
        const entries = documents.flatMap(splitAccountDocuments).map(entry => ({
          ...entry,
          credentialTarget: entry.credentialTarget ?? (entry.provider === 'openai' ? createForm.value.credentialTarget : 'codex'),
        }))
        const existingAccount = reauthorizingAccount.value
        if (existingAccount && (entries.length !== 1 || entries[0]?.credentialTarget !== 'excel'))
          throw new Error('请导入当前账号的一份 Excel 凭据')
        const items = entries.map(entry => ({
          provider: entry.provider,
          credentialTarget: entry.credentialTarget,
          data: entry.document,
          ...(createForm.value.importTemplate ? { template: accountImportTemplate(createForm.value, entry.provider), templateSettingsOverride: true } : {}),
          ...(createForm.value.qualityTemplate ? { qualityTemplate: accountImportQualityTemplate(createForm.value) } : {}),
          settings: accountImportSettings(entries.length === 1 ? createForm.value : { ...createForm.value, customName: '' }, entry.provider),
          outboundProxyId: accountCreateEgress(createForm.value, entry.provider === 'openai').outboundProxyId || undefined,
        }))
        const encoded = JSON.stringify(items, (_key, value: unknown) => isRecord(value)
          ? Object.fromEntries(Object.keys(value).sort().map(key => [key, value[key]]))
          : value)
        const codexItems = items.filter(item => item.credentialTarget !== 'excel').map(({ credentialTarget: _target, ...item }) => item)
        const excelItems = items.filter(item => item.credentialTarget === 'excel')
        const batches = accountImportBatches(codexItems)
        if (submissionKey !== encoded) {
          submissionIds = batches.map(() => generateSubmissionId())
          submittedBatches = 0
          importedExcelEntries = 0
          submissionKey = encoded
        }
        // Keep accepted Excel items out of retries when a later item or task fails.
        for (let index = importedExcelEntries; index < excelItems.length; index++) {
          const item = excelItems[index]!
          await importExcelCredential({
            data: item.data,
            ...(existingAccount
              ? { accountId: existingAccount.id, expectedRevision: existingAccount.excelCredential?.revision }
              : { settings: item.settings, outboundProxyId: item.outboundProxyId }),
          })
          if (submissionKey === encoded)
            importedExcelEntries = index + 1
        }
        // Preserve IDs and successful progress if a later submission needs retrying.
        const batchIds = submissionIds
        let lastTask: AccountImportTask | undefined
        for (let index = submittedBatches; index < batches.length; index++) {
          const task = await createAccountImportTask({ submissionId: batchIds[index]!, items: batches[index]! })
          if (submissionKey === encoded)
            submittedBatches = index + 1
          lastTask = task
        }
        showCreateModal.value = false
        if (lastTask)
          options.onImportTaskCreated(lastTask)
        if (excelItems.length)
          await options.reload()
        toast.success(existingAccount ? 'Excel 凭据已更新' : excelItems.length ? `已导入 ${excelItems.length} 份 Excel 凭据${lastTask ? '，其他凭据导入任务已创建' : ''}` : '导入任务已创建')
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
        const start = createForm.value.provider === 'openai' && createForm.value.credentialTarget === 'excel' ? startExcelOAuth : startAccountOAuth
        const result = await start({
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
        const expiresAt = Date.parse(result.expiresAt)
        if (Number.isFinite(expiresAt)) {
          oauthExpiryTimer = setTimeout(() => {
            if (createModalOpen.value && createForm.value.oauthFlowId === result.flowId && !creatingAccount.value)
              toast.warning('授权链接已过期，请重新生成')
          }, Math.max(0, Math.min(expiresAt - Date.now(), 2_147_483_647)))
        }
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
        const excel = createForm.value.provider === 'openai' && createForm.value.credentialTarget === 'excel'
        const command = {
          provider: createForm.value.provider,
          flowId: createForm.value.oauthFlowId,
          callbackUrl,
          settings: reauthorizingAccount.value ? undefined : accountImportSettings(createForm.value),
        }
        const result = excel
          ? await completeExcelOAuth({ flowId: command.flowId, callbackUrl: command.callbackUrl, settings: command.settings }).then(result => ({ accountId: result.accountIds[0] }))
          : await completeAccountOAuth(command)
        if (!result.accountId)
          throw new Error('授权返回缺少账号标识')
        let templateWarning = !reauthorizingAccount.value && excel && createForm.value.qualityTemplate
          ? 'Excel 凭据已保存；监测模板请在账号保存后单独设置'
          : ''
        if (!reauthorizingAccount.value && !excel && createForm.value.qualityTemplate) {
          try {
            const template = createForm.value.qualityTemplate
            const applied = await applyQualityTemplate({
              id: template.id,
              revision: template.revision,
              targets: [{ accountId: result.accountId, ruleId: null, revision: null }],
            })
            if (!applied[0]?.success)
              templateWarning = '账号已添加，但监测模板未应用'
          }
          catch (cause) {
            templateWarning = `账号已添加，但监测模板未应用：${errorMessage(cause)}`
          }
        }
        await finishCreate(
          reauthorizingAccount.value
            ? `${excel ? 'Excel' : createForm.value.provider === 'openai' ? 'Codex' : 'xAI'} 凭据已更新`
            : excel
              ? 'Excel 凭据已保存'
              : createForm.value.provider === 'xai'
                ? 'xAI OAuth 账号已添加'
                : 'OpenAI OAuth 账号已添加',
          templateWarning,
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

  function openReauthorizeAccount(account: AccountRow, credentialTarget: 'codex' | 'excel' = account.responsesUpstream === 'excel' ? 'excel' : 'codex') {
    if (account.provider !== 'openai' && account.provider !== 'xai')
      return
    clearSubmission()
    reauthorizingAccount.value = account
    createForm.value = {
      ...emptyAccountCreateForm(),
      provider: account.provider,
      step: 'import',
      mode: 'oauth',
      credentialTarget: account.provider === 'openai' ? credentialTarget : 'codex',
    }
    showCreateModal.value = true
    void handleAuthorizeOAuth()
  }

  function newAccountInput() {
    const account = reauthorizingAccount.value
    return {
      provider: createForm.value.provider,
      name: account?.name ?? (createForm.value.provider === 'openai' ? '' : `${createForm.value.provider} OAuth`),
    }
  }

  async function finishCreate(message: string, warning = '') {
    showCreateModal.value = false
    await options.reload()
    if (warning)
      toast.warning(warning)
    else
      toast.success(message)
  }

  watch(
    () => createForm.value.provider,
    (provider, previousProvider) => {
      createForm.value = {
        ...createForm.value,
        proxyMode: provider === 'openai' && !previousProvider && createForm.value.proxyMode === 'direct'
          ? 'inherit'
          : provider !== 'openai' && !['direct', 'proxy'].includes(createForm.value.proxyMode)
            ? 'direct'
            : createForm.value.proxyMode,
        mode: createForm.value.provider === 'batch' ? 'json' : 'oauth',
        credentialTarget: provider === 'openai' ? createForm.value.credentialTarget : 'codex',
        importTexts: { access_token: '', refresh_token: '', json: '', two_fa: '' },
        replaceExisting2fa: false,
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
      () => createForm.value.credentialTarget,
    ],
    () => {
      createForm.value.oauthFlowId = ''
      createForm.value.oauthAuthUrl = ''
      createForm.value.oauthCallback = ''
      if (createForm.value.credentialTarget === 'excel' && !['oauth', 'json', 'two_fa'].includes(createForm.value.mode))
        createForm.value.mode = 'oauth'
      if (reauthorizingAccount.value && createForm.value.credentialTarget !== 'excel')
        createForm.value.mode = 'oauth'
    },
    { flush: 'sync' },
  )

  return {
    enrollments,
    showCreateModal,
    clearCreate,
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
  if (isSupportedProvider(value.provider) && isRecord(value.document)) {
    if (value.provider !== provider)
      throw new Error('导入文件的平台与当前选择不一致')
    return [typedImportDocument(provider, value.document, value.credentialTarget)]
  }
  return [typedImportDocument(provider, value)]
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
    documents.push(typedImportDocument(provider, entry.document, entry.credentialTarget))
  }

  if (documents.length === 0)
    throw new Error('批量文件没有可导入的账号文档')
  return documents
}

function typedImportDocument(provider: ImportProvider, document: Record<string, unknown>, target?: unknown): MixedImportDocument {
  if (target !== undefined && target !== 'codex' && target !== 'excel')
    throw new Error('导入文件包含无效的凭据类型')
  const excelDocument = document.type === 'cpa-bps' || document.type === 'excel'
  const codexDocument = document.type === 'codex'
  if (target === 'codex' && excelDocument)
    throw new Error('Excel 凭据不能标记为 Codex')
  if (target === 'excel' && codexDocument)
    throw new Error('Codex 凭据不能标记为 Excel')
  const credentialTarget = excelDocument ? 'excel' : codexDocument ? 'codex' : target
  if (credentialTarget === 'excel' && provider !== 'openai')
    throw new Error('Excel 凭据仅支持 OpenAI 账号')
  return { provider, document, ...(credentialTarget ? { credentialTarget } : {}) }
}
