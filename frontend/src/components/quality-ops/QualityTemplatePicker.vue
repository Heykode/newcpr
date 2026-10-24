<script setup lang="ts">
import type { QualityRuleTemplate } from '@/api/modules/quality-ops'
import { Settings2 } from '@lucide/vue'
import { computed, onMounted, onScopeDispose, shallowRef } from 'vue'
import { useRouter } from 'vue-router'
import { getQualityTemplates } from '@/api/modules/quality-ops'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { errorMessage } from '@/utils/async'

withDefaults(defineProps<{ disabled: boolean, label?: string, manage?: boolean }>(), { label: '导入监测模板', manage: false })
const selected = defineModel<QualityRuleTemplate | null>({ required: true })
const router = useRouter()
const rows = shallowRef<QualityRuleTemplate[]>([])
const loading = shallowRef(true)
const error = shallowRef('')
let controller: AbortController | undefined
const options = computed(() => [
  { value: '', label: '不使用模板' },
  ...rows.value.map(row => ({ value: `${row.id}:${row.revision}`, label: row.name })),
])
const value = computed({
  get: () => selected.value ? `${selected.value.id}:${selected.value.revision}` : '',
  set: (key: string) => {
    const row = rows.value.find(item => `${item.id}:${item.revision}` === key)
    selected.value = row ?? null
  },
})
async function load() {
  controller?.abort()
  const request = new AbortController()
  controller = request
  loading.value = true
  error.value = ''
  try {
    const result = await getQualityTemplates({ silent: true, signal: request.signal })
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
onScopeDispose(() => controller?.abort())
</script>

<template>
  <div class="min-w-0">
    <BaseFormItem :label="label">
      <div class="flex min-w-0 items-center gap-2">
        <BaseSelect v-model="value" class="min-w-0 flex-1" :options="options" :disabled="disabled || loading" :aria-label="label" />
        <BaseIconButton v-if="manage" label="管理监测模板" :disabled="disabled" @click="router.push({ path: '/quality-ops', query: { tab: 'templates' } })">
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
  </div>
</template>
