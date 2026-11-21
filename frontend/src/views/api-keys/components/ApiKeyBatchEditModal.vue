<script setup lang="ts">
import type { AccountGroup, ApiKeyBatchChanges } from '@/api'
import type { ClientProfileConfiguration, OutboundUserAgentSettings } from '@/api/modules/outbound-user-agent'
import { computed, ref, shallowRef, watch } from 'vue'
import { batchUpdateApiKeys } from '@/api'
import { getOutboundUserAgent } from '@/api/modules/outbound-user-agent'
import AccountGroupCheckboxGrid from '@/components/AccountGroupCheckboxGrid.vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { useRequestState } from '@/composables/useRequestState'
import ClientProfileFields from '@/views/settings/components/ClientProfileFields.vue'

const props = defineProps<{ ids: string[], groups: AccountGroup[], groupLoading: boolean }>()
const emit = defineEmits<{ saved: [ids: string[]] }>()
const open = defineModel<boolean>({ required: true })
const action = useAsyncAction()
const { loading } = action
const frozenIds = shallowRef<string[]>([])
const selected = ref(new Set<string>())
const confirmAll = shallowRef(false)
const groupIds = ref<string[]>([])
const enabled = shallowRef('true')
const installationSeed = shallowRef('')
const profileMode = shallowRef('inherit')
const configuration = ref<ClientProfileConfiguration>({ client: 'cli', platform: 'linux', versionMode: 'latest' })
const profileSettings = shallowRef<OutboundUserAgentSettings | null>(null)
const profileRequest = useRequestState()
const limits = ref({ dailyLimitUsd: '', weeklyLimitUsd: '', maxConcurrency: '', requestsPerMinute: '' })
const fields = [
  { key: 'dailyLimitUsd', label: '日限额（美元）' },
  { key: 'weeklyLimitUsd', label: '周限额（美元）' },
  { key: 'maxConcurrency', label: '最大并发' },
  { key: 'requestsPerMinute', label: '每分钟请求数' },
] as const
const validation = shallowRef('')
function generateSeed() {
  installationSeed.value = globalThis.crypto.randomUUID()
}
function select(field: string, value: boolean) {
  if (value)
    selected.value.add(field)
  else selected.value.delete(field)
  validation.value = ''
}
const changes = computed<ApiKeyBatchChanges>(() => {
  const result: ApiKeyBatchChanges = {}
  for (const { key } of fields) {
    if (!selected.value.has(key))
      continue
    if (key === 'dailyLimitUsd' || key === 'weeklyLimitUsd')
      result[key] = limits.value[key].trim() || '0'
    else result[key] = Number(limits.value[key])
  }
  if (selected.value.has('groups'))
    result.groupIds = [...groupIds.value]
  if (selected.value.has('enabled'))
    result.enabled = enabled.value === 'true'
  if (selected.value.has('identity'))
    result.providerRequestProfileOverrides = { openai: profileMode.value === 'inherit' ? null : { ...configuration.value } }
  if (selected.value.has('seed'))
    result.installationSeed = installationSeed.value.trim()
  return result
})
watch(open, async (visible) => {
  profileRequest.invalidate()
  if (!visible)
    return
  frozenIds.value = [...props.ids]
  selected.value = new Set()
  groupIds.value = []
  confirmAll.value = false
  enabled.value = 'true'
  profileMode.value = 'inherit'
  configuration.value = { client: 'cli', platform: 'linux', versionMode: 'latest' }
  profileSettings.value = null
  installationSeed.value = ''
  limits.value = { dailyLimitUsd: '', weeklyLimitUsd: '', maxConcurrency: '', requestsPerMinute: '' }
  validation.value = ''
  const id = profileRequest.start()
  try {
    const settings = await getOutboundUserAgent({ signal: profileRequest.signal, silent: true })
    if (profileRequest.isCurrent(id))
      profileSettings.value = settings
  }
  catch (error) { profileRequest.fail(id, error) }
  finally { profileRequest.finish(id) }
})
async function save() {
  if (loading.value || !selected.value.size)
    return
  for (const { key, label } of fields) {
    if (!selected.value.has(key))
      continue
    const value = limits.value[key].trim() || '0'
    const valid = key === 'dailyLimitUsd' || key === 'weeklyLimitUsd'
      ? /^\d{1,10}(?:\.\d{1,10})?$/.test(value)
      : /^\d+$/.test(value) && Number.isSafeInteger(Number(value)) && Number(value) <= 1_000_000
    if (!valid) {
      validation.value = `${label}无效`
      return
    }
  }
  if (selected.value.has('seed') && !/^[\da-f]{8}-[\da-f]{4}-[\da-f]{4}-[\da-f]{4}-[\da-f]{12}$/i.test(installationSeed.value.trim())) {
    validation.value = '安装种子须为带连字符的 UUID'
    return
  }
  if (selected.value.has('groups') && !groupIds.value.length) {
    confirmAll.value = true
    return
  }
  await apply()
}
async function apply() {
  if (loading.value)
    return
  confirmAll.value = false
  const ids = [...frozenIds.value]
  const patch = JSON.parse(JSON.stringify(changes.value)) as ApiKeyBatchChanges
  await action.run(async () => {
    const result = await batchUpdateApiKeys({ ids, ...patch })
    open.value = false
    toast.success(`已更新 ${result.updated} 个 API Key`)
    emit('saved', ids)
  })
}
</script>

<template>
  <BaseModal v-model="open" title="批量编辑 API Key" :description="`已选 ${frozenIds.length} 项，仅修改勾选的字段`" size="lg" :dismissible="!loading">
    <div class="grid gap-5">
      <div class="grid gap-2">
        <BaseCheckbox :model-value="selected.has('groups')" label="修改分组" show-label :disabled="loading" @update:model-value="select('groups', $event)" />
        <AccountGroupCheckboxGrid v-if="selected.has('groups')" v-model="groupIds" :groups="groups" :loading="groupLoading" :disabled="loading" />
      </div>
      <div class="grid gap-2">
        <BaseCheckbox :model-value="selected.has('enabled')" label="修改状态" show-label :disabled="loading" @update:model-value="select('enabled', $event)" />
        <BaseSelect v-if="selected.has('enabled')" v-model="enabled" aria-label="密钥状态" :options="[{ value: 'true', label: '启用' }, { value: 'false', label: '禁用' }]" :disabled="loading" />
      </div>
      <div class="grid gap-4 sm:grid-cols-2">
        <div v-for="field in fields" :key="field.key" class="grid gap-2">
          <BaseCheckbox :model-value="selected.has(field.key)" :label="field.label" show-label :disabled="loading" @update:model-value="select(field.key, $event)" />
          <BaseInput v-model="limits[field.key]" :aria-label="field.label" placeholder="0 表示不限额" :disabled="loading || !selected.has(field.key)" />
        </div>
      </div>
      <div class="grid gap-2">
        <BaseCheckbox :model-value="selected.has('identity')" label="修改 Codex 客户端身份" show-label :disabled="loading" @update:model-value="select('identity', $event)" />
        <template v-if="selected.has('identity')">
          <BaseSelect v-model="profileMode" aria-label="客户端身份" :disabled="loading" :options="[{ value: 'inherit', label: '继承全局' }, { value: 'custom', label: '单独配置' }]" />
          <ClientProfileFields v-if="profileMode === 'custom'" v-model="configuration" :settings="profileSettings" :preview="null" :disabled="loading" />
          <span v-if="profileRequest.error.value" role="alert">{{ profileRequest.error.value }}</span>
        </template>
      </div>
      <BaseFormItem>
        <BaseCheckbox :model-value="selected.has('seed')" label="修改安装种子" show-label :disabled="loading" @update:model-value="select('seed', $event)" />
        <div v-if="selected.has('seed')" class="mt-2 flex gap-2">
          <BaseInput v-model="installationSeed" aria-label="安装种子" :disabled="loading" />
          <BaseButton :disabled="loading" @click="generateSeed()">
            生成
          </BaseButton>
        </div>
      </BaseFormItem>
      <span v-if="validation" role="alert" class="text-cp-error-text">{{ validation }}</span>
    </div>
    <template #footer>
      <BaseButton :disabled="loading" @click="open = false">
        取消
      </BaseButton>
      <BaseButton variant="primary" :loading="loading" :disabled="!selected.size || !frozenIds.length || frozenIds.length > 1000" @click="save">
        保存修改
      </BaseButton>
    </template>
  </BaseModal>
  <BaseConfirmModal v-model="confirmAll" title="允许访问全部账号" description="所选密钥将不再受分组范围限制" :loading="loading" @confirm="apply" />
</template>
