<script setup lang="ts">
import { Copy } from '@lucide/vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'

import { t } from '@/i18n'

defineProps<{
  prefix: string
  revealing: boolean
}>()

const emit = defineEmits<{
  copy: []
}>()
</script>

<template>
  <div class="flex items-center gap-2">
    <code class="block min-w-0 truncate font-mono text-cp-sm font-emphasis text-cp-text">
      {{ prefix }}••••••••••••••••
    </code>
    <BaseIconButton
      variant="ghost"
      size="sm"
      :label="t('keys.fields.copyFullKey')"
      :loading="revealing"
      :disabled="revealing"
      @click="emit('copy')"
    >
      <Copy class="size-3.5" />
    </BaseIconButton>
  </div>
</template>
