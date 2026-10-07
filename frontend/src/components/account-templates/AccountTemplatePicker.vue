<script setup lang="ts">
import type { AccountTemplate } from '@/api/modules/account-templates'
import { Settings2 } from '@lucide/vue'
import { computed, onMounted, onScopeDispose, shallowRef, watch } from 'vue'
import { getAccountTemplates } from '@/api/modules/account-templates'
import { ipv6EgressModes } from '@/api/modules/ipv6-egress'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { useAccountGroupCatalog } from '@/composables/useAccountGroupCatalog'
import { useProxyCatalog } from '@/composables/useProxyCatalog'
import { errorMessage } from '@/utils/async'
import { accountExcel403Action, excel403ActionLabel } from '@/utils/excel-settings'
import AccountTemplatesModal from './AccountTemplatesModal.vue'

withDefaults(defineProps<{ disabled: boolean, label?: string, manage?: boolean }>(), { label: '新增账号模板', manage: false })
const selected = defineModel<AccountTemplate | null>({ required: true })
const managing = shallowRef(false)
const rows = shallowRef<AccountTemplate[]>([])
const loading = shallowRef(true)
const error = shallowRef('')
let controller: AbortController | undefined
const { groups } = useAccountGroupCatalog()
const { proxies } = useProxyCatalog()
const options = computed(() => [
  { value: '', label: '不使用模板' },
  ...rows.value.map(row => ({ value: `${row.id}:${row.revision}`, label: row.config.name })),
])
const staleSelection = computed(() => !loading.value && !error.value && selected.value !== null
  && !rows.value.some(row => row.id === selected.value?.id && row.revision === selected.value?.revision))
const value = computed({
  get: () => selected.value ? `${selected.value.id}:${selected.value.revision}` : '',
  set: (key: string) => {
    const row = rows.value.find(row => `${row.id}:${row.revision}` === key)
    selected.value = row ? { ...row, config: { ...row.config, groupIds: [...row.config.groupIds] } } : null
  },
})
const groupNames = computed(() => selected.value?.config.groupIds.map(id => groups.value.find(group => group.id === id)?.name ?? `未识别分组 (${id})`).join('、') || '无分组')
const proxyName = computed(() => {
  if (selected.value?.config.preserveOutboundProxy)
    return '保持原代理'
  const id = selected.value?.config.outboundProxyId
  return id ? proxies.value.find(proxy => proxy.id === id)?.name ?? `未识别代理 (${id})` : '直连'
})
const egressLabel = computed(() => {
  const mode = selected.value?.config.egressMode
  if (mode === undefined)
    return '不修改现有策略'
  if (mode === null)
    return '继承全局策略'
  return ipv6EgressModes.find(option => option.value === mode)?.label ?? mode
})
async function load() {
  controller?.abort()
  const request = new AbortController()
  controller = request
  loading.value = true
  error.value = ''
  try {
    const result = await getAccountTemplates({ silent: true, signal: request.signal })
    if (!request.signal.aborted)
      rows.value = result
  }
  catch (cause) {
    if (!request.signal.aborted)
      error.value = errorMessage(cause)
  }
  finally {
    if (!request.signal.aborted)
      loading.value = false
  }
}
onMounted(load)
watch(managing, (open) => {
  if (!open)
    void load()
})
onScopeDispose(() => controller?.abort())
</script>

<template>
  <div class="my-4 min-w-0 border-y border-cp-border py-4">
    <BaseFormItem :label="label">
      <div class="flex min-w-0 items-center gap-2">
        <BaseSelect v-model="value" class="min-w-0 flex-1" :options="options" :disabled="disabled || loading" :aria-label="label" />
        <BaseIconButton v-if="manage" label="管理账号模板" :disabled="disabled" @click="managing = true">
          <Settings2 :size="16" />
        </BaseIconButton>
      </div>
    </BaseFormItem>
    <p v-if="error" class="mb-0 break-words text-cp-sm text-cp-error" role="alert">
      {{ error }}
    </p>
    <BaseButton v-if="error" size="sm" :disabled="disabled || loading" @click="load">
      重试加载模板
    </BaseButton>
    <p v-if="staleSelection" class="mb-3 break-words text-cp-sm text-cp-error" role="alert">
      所选模板已修改或删除，请重新选择并确认配置。
    </p>
    <dl v-if="selected" class="mb-0 grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 text-cp-sm">
      <dt>模板版本</dt><dd>{{ selected.revision }}</dd>
      <template v-if="selected.config.turnStateInjectionEnabled != null">
        <dt>State</dt><dd>{{ selected.config.turnStateInjectionEnabled ? '开启' : '关闭' }}</dd>
      </template>
      <template v-if="selected.config.modelAccess">
        <dt>模型限制</dt><dd class="min-w-0 break-all">
          {{ selected.config.modelAccess.mode === 'all' ? '不限制' : `${selected.config.modelAccess.mode === 'allowlist' ? '白名单' : '黑名单'}：${selected.config.modelAccess.models.join(', ')}` }}
        </dd>
      </template>
      <dt>IPv6 出口策略</dt><dd>{{ egressLabel }}</dd>
      <template v-if="selected.config.excelIgnoreEncryptedContent != null">
        <dt>忽略加密历史</dt><dd>{{ selected.config.excelIgnoreEncryptedContent ? '开启（有损省略）' : '关闭' }}</dd>
      </template>
      <template v-if="selected.config.excel403Action != null || selected.config.excelAutoDisableOn403 != null">
        <dt>Excel遇到HTTP 403</dt>
        <dd>{{ excel403ActionLabel(accountExcel403Action(selected.config)) }}</dd>
      </template>
      <template v-if="selected.config.excelRecovery">
        <dt>Excel 暂停恢复探测</dt>
        <dd>{{ selected.config.excelRecovery.enabled ? `开启，每 ${selected.config.excelRecovery.intervalMinutes} 分钟` : '关闭' }}</dd>
      </template>
      <dt class="text-cp-text-secondary">
        调度
      </dt><dd class="m-0">
        {{ selected.config.enabled ? '启用' : '暂停' }}
      </dd>
      <template v-if="selected.config.responsesUpstream != null">
        <dt class="text-cp-text-secondary">
          Excel 入口
        </dt>
        <dd class="m-0">
          {{ selected.config.responsesUpstream === 'excel' ? '开启' : '关闭' }}
        </dd>
      </template>
      <template v-if="selected.config.excelModelsFollowGlobal != null || selected.config.excelModels != null">
        <dt class="text-cp-text-secondary">
          Excel 模型
        </dt>
        <dd class="m-0 break-all">
          {{ selected.config.excelModelsFollowGlobal ? '跟随全局' : (selected.config.excelModels ?? []).join(', ') || '无' }}
        </dd>
      </template>
      <template v-if="selected.config.excelCacheCreationAsInput != null">
        <dt>缓存写入按输入计费</dt>
        <dd>{{ selected.config.excelCacheCreationAsInput ? '开启' : '关闭' }}</dd>
      </template>
      <dt class="text-cp-text-secondary">
        账号并发
      </dt><dd class="m-0">
        {{ selected.config.concurrencyLimit ?? '默认值' }}
      </dd>
      <dt class="text-cp-text-secondary">
        权重
      </dt><dd class="m-0">
        {{ selected.config.weight }}
      </dd>
      <dt class="text-cp-text-secondary">
        所属分组
      </dt><dd class="m-0 break-all">
        {{ groupNames }}
      </dd>
      <dt class="text-cp-text-secondary">
        出站代理
      </dt><dd class="m-0 break-all">
        {{ proxyName }}
      </dd>
    </dl>
    <AccountTemplatesModal v-if="manage" v-model="managing" />
  </div>
</template>
