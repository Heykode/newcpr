<script setup lang="ts">
import type { OpenAiRequestCountry } from '@/api/modules/settings'
import { computed } from 'vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'

defineProps<{ disabled: boolean }>()
const guardian = defineModel<boolean>('guardian', { required: true })
const country = defineModel<OpenAiRequestCountry>('country', { required: true })
const mode = computed({
  get: () => country.value.mode,
  set: (value: OpenAiRequestCountry['mode']) => {
    country.value = value === 'custom' ? { mode: value, country: '' } : { mode: value }
  },
})
</script>

<template>
  <BaseCard title="Guardian 审核模型" description="同步审核优先使用账号可用的 codex-auto-review">
    <BaseSwitch v-model="guardian" label="使用专用审核模型" show-label :disabled="disabled" />
  </BaseCard>
  <BaseCard title="请求国家">
    <div class="grid gap-4 sm:grid-cols-2">
      <BaseFormItem label="发送策略">
        <BaseSelect
          v-model="mode" aria-label="请求国家策略" :disabled="disabled" :options="[
            { value: 'follow_location', label: '跟随位置' },
            { value: 'default', label: '发送 null' },
            { value: 'passthrough', label: '透传客户端' },
            { value: 'custom', label: '自定义国家' },
          ]"
        />
      </BaseFormItem>
      <BaseFormItem v-if="country.mode === 'custom'" label="国家代码" prop="openaiRequestCountry.country" :rules="{ required: true, pattern: /^[A-Z]{2}$/, message: '请输入两位大写国家代码', trigger: 'blur' }">
        <BaseInput v-model="country.country" aria-label="国家代码" placeholder="US" :disabled="disabled" />
      </BaseFormItem>
    </div>
    <p class="mt-3 text-cp-sm text-cp-text-secondary">
      跟随位置依次使用账号代理位置、已启用的全局位置，无位置时发送 null
    </p>
  </BaseCard>
</template>
