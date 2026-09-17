<script setup lang="ts">
import type { getAccounts } from '@/api'
import { KeyRound } from '@lucide/vue'
import { computed } from 'vue'

import ProviderIconGroup from '@/components/ProviderIconGroup.vue'
import { stablePresetVisualToneClass } from '../utils/visualTone'
import AccountPlanBadge from './AccountPlanBadge.vue'

type AccountRow = Awaited<ReturnType<typeof getAccounts>>['items'][number]
type AccountIdentity = Pick<AccountRow, 'id' | 'email' | 'planType' | 'planTypeDisplay'>
  & Partial<Pick<AccountRow, 'provider' | 'authenticationKind'>>
  & Partial<Pick<AccountRow, 'accountId'>>

const props = withDefaults(
  defineProps<{
    account: AccountIdentity
    hasTotp?: boolean
    size?: 'md' | 'lg'
    showPlan?: boolean
    titleMode?: 'local-part' | 'email'
    metaPosition?: 'title' | 'secondary'
    metaSize?: 'xs' | 'sm'
  }>(),
  {
    size: 'md',
    hasTotp: false,
    showPlan: false,
    titleMode: 'local-part',
    metaPosition: 'title',
    metaSize: 'sm',
  },
)

const emailText = computed(() => {
  const email = props.account.email?.trim()
  if (email)
    return email
  return '未命名账号'
})

const displayTitle = computed(() =>
  props.titleMode === 'email' ? emailText.value : emailText.value.split('@')[0],
)

const secondaryText = computed(() =>
  props.titleMode === 'email' ? null : emailText.value,
)

const avatarSizeClass = computed(() =>
  props.size === 'lg' ? 'size-10 text-cp-xl' : 'size-9 text-cp',
)

const secondaryClass = computed(() =>
  props.size === 'lg'
    ? 'mt-1 text-cp-sm text-cp-text-secondary'
    : 'mt-0.5 font-mono text-cp-xs text-cp-text-quaternary',
)

const metaGapClass = computed(() => props.metaSize === 'xs' ? 'gap-1' : 'gap-1.5')

const avatarToneClass = computed(() => {
  const identity = props.account.id || props.account.email || displayTitle.value
  return stablePresetVisualToneClass(identity)
})
</script>

<template>
  <div class="flex min-w-0 items-center gap-3">
    <span class="relative inline-flex shrink-0">
      <span
        data-swipe-select-handle
        class="inline-flex items-center justify-center rounded-lg"
        :class="[avatarSizeClass, avatarToneClass]"
      >
        <ProviderIconGroup
          v-if="account.provider"
          :provider="account.provider"
          size="md"
        />
        <span v-else class="font-extrabold">{{ displayTitle.slice(0, 1).toUpperCase() }}</span>
      </span>
      <span
        v-if="hasTotp"
        data-account-totp-mark
        data-swipe-select-handle
        class="absolute -left-1 -top-1 z-10 inline-flex size-4 items-center justify-center rounded-full border-2 border-cp-bg-container bg-cp-primary text-white shadow-sm"
        title="已配置 2FA，可用于失效重登"
        aria-label="已配置 2FA，可用于失效重登"
        role="img"
      >
        <KeyRound class="size-2.5" :stroke-width="2.5" />
      </span>
    </span>
    <div class="min-w-0 flex-1" data-swipe-select-ignore>
      <div class="flex min-w-0 items-center gap-2">
        <span class="min-w-0 flex-1 truncate text-cp font-heavy text-cp-text">
          {{ displayTitle }}
        </span>
        <span
          v-if="metaPosition === 'title' && (showPlan || $slots.meta)"
          class="inline-flex shrink-0 items-center justify-end"
          :class="metaGapClass"
        >
          <slot name="meta" />
          <AccountPlanBadge v-if="showPlan" :plan-type="account.planType" :plan-type-display="account.planTypeDisplay" :size="metaSize" />
        </span>
      </div>
      <div
        v-if="metaPosition === 'secondary' && (showPlan || $slots.meta)"
        class="mt-0.5 inline-flex min-w-0 items-center"
        :class="metaGapClass"
      >
        <slot name="meta" />
        <AccountPlanBadge v-if="showPlan" :plan-type="account.planType" :plan-type-display="account.planTypeDisplay" :size="metaSize" />
      </div>
      <div v-else-if="secondaryText" class="truncate font-emphasis" :class="secondaryClass">
        {{ secondaryText }}
      </div>
    </div>
  </div>
</template>
