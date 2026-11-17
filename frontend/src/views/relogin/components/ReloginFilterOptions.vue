<script setup lang="ts">
import { Check } from '@lucide/vue'

defineProps<{ options: { value: string, label: string }[], label: string }>()
const model = defineModel<string>({ required: true })
</script>

<template>
  <div class="grid gap-0.5" :aria-label="label">
    <span class="px-2 pb-1 text-cp-xs text-cp-text-tertiary">{{ label }}</span>
    <button v-for="option in options" :key="option.value" type="button" class="flex min-h-8 items-center justify-between gap-3 rounded-cp px-2 py-1.5 text-left text-cp-sm" :class="model === option.value ? 'bg-cp-primary-container text-cp-primary-on-container' : 'text-cp-text hover:bg-cp-fill-tertiary'" :aria-pressed="model === option.value" @click="model = option.value">
      <span>{{ option.label }}</span><Check v-if="model === option.value" class="size-3.5 shrink-0" />
    </button>
  </div>
</template>
