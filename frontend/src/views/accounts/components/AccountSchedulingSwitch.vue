<script setup lang="ts">
import type { AccountStatus } from '@/api'
import { computed } from 'vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'

const props = defineProps<{
  enabled: boolean
  status: AccountStatus
  loading?: boolean
}>()
const emit = defineEmits<{ change: [enabled: boolean] }>()
const automaticallyStopped = computed(() => props.enabled && props.status === 'error')
const label = computed(() => automaticallyStopped.value
  ? '自动停调；凭据恢复后自动解除。手动暂停请使用账号设置。'
  : props.enabled ? '暂停账号调度' : '启用账号调度')

function change(enabled: boolean) {
  if (!props.loading && !automaticallyStopped.value)
    emit('change', enabled)
}
</script>

<template>
  <div class="inline-flex min-h-5 items-center justify-center gap-1" :title="label">
    <BaseSwitch
      :model-value="enabled && !automaticallyStopped"
      :label="label"
      :disabled="loading || automaticallyStopped"
      @update:model-value="change"
    />
    <span v-if="automaticallyStopped" class="whitespace-nowrap text-[10px] text-cp-text-secondary">自动停调</span>
  </div>
</template>
