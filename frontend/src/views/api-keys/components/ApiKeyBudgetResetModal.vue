<script setup lang="ts">
import type { ApiKey, ApiKeyBudgetPeriod } from '@/api'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import BaseSegmented from '@/components/base/BaseSegmented.vue'

defineProps<{ apiKey: ApiKey | null, loading: boolean }>()
const emit = defineEmits<{ confirm: [] }>()
const open = defineModel<boolean>({ default: false })
const period = defineModel<ApiKeyBudgetPeriod>('period', { default: 'all' })
function selectPeriod(value: string) {
  if (value === 'daily' || value === 'weekly' || value === 'all')
    period.value = value
}
</script>

<template>
  <BaseConfirmModal
    v-model="open"
    title="重置已用额度"
    description="不改变限额和窗口到期时间，历史费用记录保留。重置后完成的请求仍会计费。"
    confirm-text="确认重置"
    :loading="loading"
    :confirm-disabled="!apiKey"
    @confirm="emit('confirm')"
  >
    <p class="mt-0 break-all">
      {{ apiKey?.name }}
    </p>
    <BaseSegmented
      :model-value="period"
      label="重置周期"
      :disabled="loading"
      :options="[
        { label: '日额度', value: 'daily' },
        { label: '周额度', value: 'weekly' },
        { label: '日与周', value: 'all' },
      ]"
      @update:model-value="selectPeriod"
    />
  </BaseConfirmModal>
</template>
