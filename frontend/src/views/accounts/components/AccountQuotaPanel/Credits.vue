<script setup lang="ts">
import type { Account } from '@/api'
import { computed, shallowRef } from 'vue'
import { batchUpdateAccounts } from '@/api'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { t } from '@/i18n'

defineOptions({ inheritAttrs: false })

const props = defineProps<{
  account: Account
  editable?: boolean
  compact?: boolean
}>()

const emit = defineEmits<{ accountUpdated: [account: Account] }>()
const { loading, run } = useAsyncAction()
const pending = shallowRef<boolean | null>(null)
const credits = computed(() => props.account.quota.credits)

async function toggleCredits(enabled: boolean) {
  const accountId = props.account.id
  await run(async () => {
    pending.value = enabled
    await batchUpdateAccounts({ accountIds: [accountId], autoUseQuotaCredits: enabled })
    if (props.account.id === accountId)
      emit('accountUpdated', { ...props.account, autoUseQuotaCredits: enabled })
  })
  pending.value = null
}

const integerFormatter = new Intl.NumberFormat('zh-CN')
const balanceDisplay = computed(() => {
  const balance = credits.value?.balance?.trim()
  const decimal = balance?.match(/^(-?)(\d+)(?:\.(\d+))?$/)
  if (!decimal)
    return null
  const [, sign, integer, fraction] = decimal
  const grouped = `${sign}${integerFormatter.format(BigInt(integer!))}`
  return fraction ? `${grouped}.${fraction}` : grouped
})
const value = computed(() => {
  if (credits.value?.unlimited)
    return '无限'
  if (balanceDisplay.value !== null)
    return balanceDisplay.value
  if (credits.value && !credits.value.hasCredits)
    return '暂无可用点数'
  return '未提供余额'
})
const hasBalance = computed(() => !credits.value?.unlimited && balanceDisplay.value !== null)
</script>

<template>
  <div
    v-bind="$attrs"
    class="flex min-w-0 flex-wrap items-baseline gap-y-1"
    :class="compact ? 'justify-start gap-x-1.5 text-[10px]' : 'justify-between gap-x-4 rounded-cp bg-cp-fill-quaternary px-4 py-3.5 text-cp-sm'"
    aria-label="Codex 点数"
  >
    <span class="font-heavy text-cp-text">Codex 点数</span>
    <span class="flex min-w-0 items-baseline gap-1.5">
      <strong
        class="min-w-0 text-right font-heavy wrap-anywhere"
        :class="hasBalance ? 'font-mono tabular-nums text-cp-text' : 'text-cp-text-secondary'"
      >
        {{ value }}
      </strong>
      <span v-if="hasBalance" class="shrink-0 text-cp-text-tertiary" :class="compact ? 'text-[10px]' : 'text-cp-xs'">{{ t('accounts.quota.point') }}</span>
    </span>
  </div>
  <div v-if="editable && account.provider === 'openai' && account.authenticationKind === 'oauth'" class="flex items-center justify-between gap-4 rounded-cp bg-cp-fill-quaternary px-4 py-3.5">
    <span class="min-w-0">
      <span class="block text-cp-sm font-heavy">{{ t('accounts.quota.autoUsePoints') }}</span>
      <span class="mt-1 block text-cp-xs text-cp-text-secondary">{{ t('accounts.quota.autoUsePointsHelp') }}</span>
    </span>
    <BaseSwitch :model-value="pending ?? account.autoUseQuotaCredits" :label="t('accounts.quota.autoUsePoints')" :disabled="loading" @update:model-value="toggleCredits" />
  </div>
</template>
