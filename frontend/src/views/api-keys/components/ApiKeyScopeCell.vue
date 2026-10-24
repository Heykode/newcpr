<script setup lang="ts">
import type { ApiKey } from '@/api'
import AccountGroupMarks from '@/components/AccountGroupMarks.vue'

import { t } from '@/i18n'

defineProps<{
  apiKey: ApiKey
}>()
</script>

<template>
  <div class="grid w-full justify-items-center gap-1.5">
    <div v-if="apiKey.routingScope === 'all'" class="flex items-center justify-center gap-2">
      <span class="inline-flex h-6 items-center rounded-lg bg-cp-warning-container px-2 text-cp-xs font-bold text-cp-warning-on-container">
        {{ t('keys.fields.allAccounts') }}
      </span>
    </div>
    <template v-else>
      <AccountGroupMarks :groups="apiKey.groups" />
    </template>
  </div>
</template>
