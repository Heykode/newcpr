<script setup lang="ts">
import type { dashboardSnapshotView, dashboardTrendView } from '../composables/useDashboard'
import type { DashboardTrendKind } from '@/api/modules/dashboard'
import { RefreshCw } from '@lucide/vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'

import BasePageHeader from '@/components/base/BasePageHeader.vue'
import { t } from '@/i18n'

import AccountOverviewCard from './AccountOverviewCard.vue'
import DashboardHeartbeat from './DashboardHeartbeat.vue'
import MetricCard from './MetricCard.vue'
import RequestHealthTimelineCard from './RequestHealthTimelineCard.vue'
import RequestTrendCard from './RequestTrendCard.vue'
import UsageRecordCard from './UsageRecordCard.vue'
import WireProfileCard from './WireProfileCard.vue'

type DashboardSnapshotView = ReturnType<typeof dashboardSnapshotView>
type DashboardTrendView = ReturnType<typeof dashboardTrendView>

withDefaults(
  defineProps<{
    loading?: boolean
    refreshing?: boolean
    lastRefreshedAt?: string
    metrics: DashboardSnapshotView['metrics']
    trendPoints: DashboardTrendView['points']
    trendSummary: DashboardTrendView['summary']
    trendLoading?: boolean
    trendError?: string
    healthTimeline: DashboardSnapshotView['healthTimeline']
    accountUsage: DashboardSnapshotView['accountUsage']
    wireProfiles: DashboardSnapshotView['wireProfiles']
    usageRecords: DashboardSnapshotView['usageRecords']
    poolSummary: DashboardSnapshotView['poolSummary']
    capacityInfo: DashboardSnapshotView['capacityInfo']
    rotationStrategy: DashboardSnapshotView['rotationStrategy']
  }>(),
  {
    loading: false,
    refreshing: false,
    lastRefreshedAt: '',
    trendLoading: false,
    trendError: '',
  },
)

const emit = defineEmits<{
  refresh: []
  trendChange: [kind: DashboardTrendKind]
}>()

const trendKind = defineModel<DashboardTrendKind>('trendKind', { required: true })
</script>

<template>
  <div class="w-full">
    <BasePageHeader :title="t('dashboard.title')">
      <template #description>
        <span>{{ t('dashboard.today') }}</span>
        <DashboardHeartbeat :updated-at="lastRefreshedAt" />
      </template>
      <template #actions>
        <BaseIconButton
          class="text-cp-primary-text"
          size="md"
          :label="t('dashboard.refresh.action')"
          :loading="loading || refreshing"
          :disabled="loading || refreshing"
          @click="emit('refresh')"
        >
          <template #loading>
            <RefreshCw class="animate-spin motion-reduce:animate-none" :size="19" />
          </template>
          <RefreshCw :size="19" />
        </BaseIconButton>
      </template>
    </BasePageHeader>

    <section
      class="mt-6 grid grid-cols-1 gap-4 md:grid-cols-2 2xl:grid-cols-4 2xl:gap-6"
      :aria-label="t('dashboard.metrics.title')"
    >
      <MetricCard v-for="metric in metrics" :key="metric.title" :metric="metric" />
    </section>

    <section
      class="mt-6 grid grid-cols-1 gap-6 2xl:grid-cols-[minmax(0,948fr)_minmax(0,608fr)] 2xl:gap-7"
    >
      <RequestTrendCard
        v-model:kind="trendKind"
        :points="trendPoints"
        :summary="trendSummary"
        :loading="trendLoading"
        :error="trendError"
        @trend-change="emit('trendChange', $event)"
      />
      <WireProfileCard :profiles="wireProfiles" />
    </section>

    <AccountOverviewCard
      :accounts="accountUsage"
      :pool="poolSummary"
      :capacity="capacityInfo"
      :rotation-strategy="rotationStrategy"
      class="mt-6"
    />

    <RequestHealthTimelineCard :timeline="healthTimeline" class="mt-6" />

    <UsageRecordCard :rows="usageRecords" class="mt-6" />
  </div>
</template>
