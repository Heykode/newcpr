<script setup lang="ts">
import type { RequestLocation } from '@/api/modules/proxies'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import { t } from '@/i18n'

defineProps<{ disabled?: boolean }>()
const location = defineModel<RequestLocation>({ required: true })
</script>

<template>
  <BaseForm class="grid min-w-0 gap-4 sm:grid-cols-2">
    <BaseFormItem :label="t('common.location.countryCode')">
      <BaseInput v-model="location.country" :aria-label="t('common.location.countryCode')" maxlength="2" :disabled="disabled" />
    </BaseFormItem>
    <BaseFormItem :label="t('common.location.region')">
      <BaseInput v-model="location.region" :aria-label="t('common.location.region')" maxlength="128" :disabled="disabled" />
    </BaseFormItem>
    <BaseFormItem :label="t('common.location.city')">
      <BaseInput v-model="location.city" :aria-label="t('common.location.city')" maxlength="128" :disabled="disabled" />
    </BaseFormItem>
    <BaseFormItem label="时区">
      <BaseInput v-model="location.timezone" aria-label="时区" :disabled="disabled" />
    </BaseFormItem>
  </BaseForm>
</template>
