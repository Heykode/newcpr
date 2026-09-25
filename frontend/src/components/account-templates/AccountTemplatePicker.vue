<script setup lang="ts">
import type { AccountTemplate } from '@/api/modules/account-templates'
import { computed, onMounted, onScopeDispose, shallowRef } from 'vue'
import { getAccountTemplates } from '@/api/modules/account-templates'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { useAccountGroupCatalog } from '@/composables/useAccountGroupCatalog'
import { useProxyCatalog } from '@/composables/useProxyCatalog'
import { errorMessage } from '@/utils/async'

defineProps<{ disabled: boolean }>()
const selected = defineModel<AccountTemplate | null>({ required: true })
const rows = shallowRef<AccountTemplate[]>([])
const loading = shallowRef(true)
const error = shallowRef('')
const controller = new AbortController()
const { groups } = useAccountGroupCatalog()
const { proxies } = useProxyCatalog()
const options = computed(() => [
  { value: '', label: '不使用模板' },
  ...rows.value.map(row => ({ value: row.id, label: row.config.name })),
])
const value = computed({
  get: () => selected.value?.id ?? '',
  set: (id: string) => {
    const row = rows.value.find(row => row.id === id)
    selected.value = row ? { ...row, config: { ...row.config, groupIds: [...row.config.groupIds] } } : null
  },
})
const groupNames = computed(() => selected.value?.config.groupIds.map(id => groups.value.find(group => group.id === id)?.name ?? `未识别分组 (${id})`).join('、') || '无分组')
const proxyName = computed(() => {
  const id = selected.value?.config.outboundProxyId
  return id ? proxies.value.find(proxy => proxy.id === id)?.name ?? `未识别代理 (${id})` : '直连'
})
onMounted(async () => {
  try {
    const result = await getAccountTemplates({ silent: true, signal: controller.signal })
    if (!controller.signal.aborted)
      rows.value = result
  }
  catch (cause) {
    if (!controller.signal.aborted)
      error.value = errorMessage(cause)
  }
  finally { loading.value = false }
})
onScopeDispose(() => controller.abort())
</script>

<template>
  <div class="my-4 min-w-0 border-y border-cp-border py-4">
    <BaseFormItem label="新增账号模板">
      <BaseSelect v-model="value" class="w-full" :options="options" :disabled="disabled || loading" aria-label="新增账号模板" />
    </BaseFormItem>
    <p v-if="error" class="mb-0 break-words text-cp-sm text-cp-error" role="alert">
      {{ error }}
    </p>
    <dl v-if="selected" class="mb-0 grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 text-cp-sm">
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
  </div>
</template>
