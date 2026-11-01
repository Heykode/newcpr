<script setup lang="ts">
import type { RequestLocation } from '@/api/modules/proxies'
import type { FormRule } from '@/components/base/BaseForm/types'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import { t } from '@/i18n'

defineProps<{ disabled?: boolean, formProp?: string }>()
const requiredRule: FormRule = { required: true, whitespace: true, trigger: ['blur', 'change'], message: '请填写此项' }
const countryRules: FormRule[] = [requiredRule, { pattern: /^[A-Z]{2}$/, transform: value => value.trim().toUpperCase(), trigger: ['blur', 'change'], message: '请填写两位国家代码' }]
const location = defineModel<RequestLocation>({ required: true })
</script>

<template>
  <BaseForm class="grid min-w-0 gap-4 sm:grid-cols-2">
    <BaseFormItem :prop="formProp ? `${formProp}.country` : undefined" :rules="countryRules" :label="t('common.location.countryCode')">
      <BaseInput v-model="location.country" :aria-label="t('common.location.countryCode')" maxlength="2" :disabled="disabled" />
    </BaseFormItem>
    <BaseFormItem :prop="formProp ? `${formProp}.region` : undefined" :rules="requiredRule" :label="t('common.location.region')">
      <BaseInput v-model="location.region" :aria-label="t('common.location.region')" maxlength="128" :disabled="disabled" />
    </BaseFormItem>
    <BaseFormItem :prop="formProp ? `${formProp}.city` : undefined" :rules="requiredRule" :label="t('common.location.city')">
      <BaseInput v-model="location.city" :aria-label="t('common.location.city')" maxlength="128" :disabled="disabled" />
    </BaseFormItem>
    <BaseFormItem :prop="formProp ? `${formProp}.timezone` : undefined" :rules="requiredRule" label="时区">
      <BaseInput v-model="location.timezone" aria-label="时区" :disabled="disabled" />
    </BaseFormItem>
  </BaseForm>
</template>
