<script setup lang="ts">
import type { OpenAiRequestCountry } from '@/api/modules/settings'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import RequestCountryCard from './RequestCountryCard.vue'

defineProps<{ disabled: boolean }>()
const guardian = defineModel<boolean>('guardian', { required: true })
const country = defineModel<OpenAiRequestCountry>('country', { required: true })
</script>

<template>
  <BaseCard title="Guardian 审核模型" description="同步审核优先使用账号可用的 codex-auto-review">
    <BaseSwitch v-model="guardian" label="使用专用审核模型" show-label :disabled="disabled" />
  </BaseCard>
  <RequestCountryCard v-model="country" :disabled="disabled" />
</template>
