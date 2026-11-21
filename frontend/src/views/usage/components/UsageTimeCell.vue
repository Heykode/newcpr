<script setup lang="ts">
import { computed } from 'vue'

const props = defineProps<{ value: string, datetime: string }>()
const parts = computed(() => props.value?.split(' '))
</script>

<template>
  <time v-if="parts?.length === 2" :datetime="datetime" :title="value" class="grid gap-1 font-mono tabular-nums">
    <span class="text-cp-sm font-bold text-cp-text">{{ parts[1] }}</span>
    <span class="text-cp-xs text-cp-text-tertiary">{{ parts[0] }}</span>
  </time>
  <span v-else class="block truncate font-mono text-cp-sm leading-none font-bold text-cp-text-secondary" :title="value">
    {{ value || '—' }}
  </span>
</template>
