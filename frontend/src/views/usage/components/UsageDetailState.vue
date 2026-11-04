<script setup lang="ts">
import { RefreshCw } from '@lucide/vue'
import BaseButton from '@/components/base/BaseButton.vue'

defineProps<{ loading?: boolean, error?: string }>()
defineEmits<{ retry: [] }>()
</script>

<template>
  <div v-if="loading" class="grid min-h-48 place-content-center text-cp-text-secondary" role="status" aria-live="polite" aria-busy="true">
    正在加载详情…
  </div>
  <div v-else-if="error" class="grid min-h-48 place-content-center justify-items-center gap-4" role="alert">
    <p class="whitespace-pre-wrap break-all">
      {{ error }}
    </p>
    <BaseButton @click="$emit('retry')">
      <RefreshCw class="size-4" />重试
    </BaseButton>
  </div>
</template>
