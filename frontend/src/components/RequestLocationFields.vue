<script setup lang="ts">
import type { RequestLocation } from '@/api/modules/proxies'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseInput from '@/components/base/BaseInput.vue'

defineProps<{ disabled?: boolean }>()
const location = defineModel<RequestLocation>({ required: true })
</script>

<template>
  <BaseForm class="grid min-w-0 gap-4 sm:grid-cols-2">
    <BaseFormItem label="国家代码">
      <BaseInput v-model="location.country" aria-label="国家代码" maxlength="2" :disabled="disabled" />
    </BaseFormItem>
    <BaseFormItem label="地区">
      <BaseInput v-model="location.region" aria-label="地区" maxlength="128" :disabled="disabled" />
    </BaseFormItem>
    <BaseFormItem label="城市">
      <BaseInput v-model="location.city" aria-label="城市" maxlength="128" :disabled="disabled" />
    </BaseFormItem>
    <BaseFormItem label="时区">
      <BaseInput v-model="location.timezone" aria-label="时区" :disabled="disabled" />
    </BaseFormItem>
  </BaseForm>
</template>
