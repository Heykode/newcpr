<script setup lang="ts">
import type { AccountRow } from '../../constants'
import { RefreshCw, UserRound } from '@lucide/vue'
import { computed, shallowRef } from 'vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'

import { t } from '@/i18n'
import { formatRelativeTime } from '@/utils/relative-time'
import { groupedAccountQuotaWindows, orderedPanelQuotaWindows, panelQuotaWindows } from '../../constants'
import AccountPlanBadge from '../AccountPlanBadge.vue'
import AccountProfileModal from '../AccountProfileModal/index.vue'
import AccountQuotaOverdraftStatus from '../AccountQuotaOverdraft/Status.vue'
import AccountQuotaCredits from './Credits.vue'
import AccountQuotaPanelEntry from './Entry.vue'
import QuotaSettings from './QuotaSettings.vue'
import AccountResetCredits from './ResetCredits.vue'

const props = defineProps<{
  account: AccountRow
  refreshing: boolean
  mobile?: boolean
}>()

const emit = defineEmits<{
  refreshQuota: [accountId: string]
  accountUpdated: [account: AccountRow]
}>()

const quotaEntries = computed(() => groupedAccountQuotaWindows(
  orderedPanelQuotaWindows(panelQuotaWindows(props.account.quota.windows)),
))
const profileOpen = shallowRef(false)
const lastRefreshed = computed(() => props.account.quota.refreshedAtRelative
  ? t('accounts.quota.lastRefreshed', {
      refreshedAtDisplay: formatRelativeTime(props.account.quota.refreshedAtRelative, props.account.quota.refreshedAtDisplay),
    })
  : undefined)
</script>

<template>
  <section class="flex min-h-0 flex-col" :class="mobile ? 'min-w-0' : 'rounded-lg bg-cp-bg-container p-4 shadow-cp-tertiary'">
    <div class="mb-3 flex shrink-0 items-start justify-between gap-3">
      <div class="min-w-0">
        <h3 class="m-0 text-cp-lg font-heavy text-cp-text">
          {{ t('accounts.quota.accountLimit') }}
        </h3>
        <p
          v-if="account.planType || lastRefreshed"
          class="m-0 mt-1 flex min-w-0 items-center gap-1.5 text-cp-xs font-emphasis text-cp-text-secondary"
          :class="mobile ? 'flex-wrap break-all' : undefined"
        >
          <template v-if="account.planType">
            <AccountPlanBadge :plan-type="account.planType" :plan-type-display="account.planTypeDisplay" size="sm" />
            <span v-if="lastRefreshed">·</span>
          </template>
          <span v-if="lastRefreshed">{{ lastRefreshed }}</span>
        </p>
      </div>
      <div class="flex shrink-0 items-center gap-0.5">
        <BaseIconButton
          v-if="account.provider === 'openai'"
          :label="t('accounts.quota.viewPersonalInformation')"
          size="sm"
          variant="ghost"
          :pressed="profileOpen"
          @click="profileOpen = true"
        >
          <UserRound class="size-3.5" />
        </BaseIconButton>
        <QuotaSettings v-if="account.quotaSettingsSupported" :key="account.id" :account="account" @account-updated="emit('accountUpdated', $event)" />
        <AccountResetCredits
          v-if="account.provider === 'openai'"
          :account="account"
          @account-updated="emit('accountUpdated', $event)"
        />
        <BaseIconButton
          variant="ghost"
          size="sm"
          :label="t('accounts.quotaForecast.refreshQuota')"
          :loading="refreshing"
          :disabled="refreshing"
          @click="emit('refreshQuota', account.id)"
        >
          <template #loading>
            <RefreshCw class="size-3.5 animate-spin motion-reduce:animate-none" />
          </template>
          <RefreshCw class="size-3.5" />
        </BaseIconButton>
      </div>
    </div>

    <div class="grid min-h-0 gap-3">
      <AccountQuotaCredits
        v-if="account.provider === 'openai' || account.provider === 'antigravity'"
        :editable="account.provider === 'antigravity'"
        :class="mobile ? 'w-full min-w-0' : 'max-sm:w-[calc(100vw-5.5rem)]'"
        :account="account"
        @account-updated="emit('accountUpdated', $event)"
      />
      <AccountQuotaOverdraftStatus
        v-if="account.provider === 'openai' && account.quota.overdraft"
        :state="account.quota.overdraft"
      />
      <AccountQuotaPanelEntry
        v-for="entry in quotaEntries"
        :key="entry.key"
        :label="entry.label"
        :overdraft="account.quota.overdraft"
        :windows="entry.windows"
      />
      <p v-if="quotaEntries.length === 0" class="m-0 text-cp-sm font-emphasis text-cp-text-secondary">
        {{ t('accounts.quota.unobserved') }}
      </p>
    </div>
  </section>

  <AccountProfileModal
    v-if="account.provider === 'openai'"
    v-model="profileOpen"
    :account="account"
  />
</template>
