<script setup lang="ts">
import type { AccountPurchaseCost } from '@/api/modules/accounts'
import { purchaseCostLabel, purchaseCostTitle } from '../utils/purchaseCost'

defineProps<{ cost?: AccountPurchaseCost | null }>()
</script>

<template>
  <div class="min-w-0 text-center tabular-nums" :title="purchaseCostTitle(cost)">
    <span data-account-breakeven>{{ purchaseCostLabel(cost) }}</span>
  </div>
</template>
