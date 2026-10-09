<script setup lang="ts">
import type { AccountPurchaseCost } from '@/api/modules/accounts'
import { purchaseCostLabel, purchaseCostTitle } from '../utils/purchaseCost'

withDefaults(defineProps<{
  cost?: AccountPurchaseCost | null
  align?: 'center' | 'right'
}>(), {
  align: 'center',
})
</script>

<template>
  <div class="min-w-0 tabular-nums" :class="align === 'right' ? 'text-right' : 'text-center'" :title="purchaseCostTitle(cost)">
    <span data-account-breakeven>{{ purchaseCostLabel(cost) }}</span>
  </div>
</template>
