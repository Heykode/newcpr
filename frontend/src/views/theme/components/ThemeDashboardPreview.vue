<script setup lang="ts">
import type { DashboardTrendKind } from '@/api/modules/dashboard'
import { computed, shallowRef } from 'vue'

import DashboardContent from '@/views/dashboard/components/DashboardContent.vue'
import {
  dashboardSnapshotView,
  dashboardTrendView,
} from '@/views/dashboard/composables/useDashboard'

import { createThemeDashboardSummary } from '../fixtures/dashboard'

const summary = computed(createThemeDashboardSummary)
const snapshot = computed(() => dashboardSnapshotView(summary.value))
const trend = computed(() => dashboardTrendView(summary.value.trend))
const trendKind = shallowRef<DashboardTrendKind>('usage')
</script>

<template>
  <DashboardContent
    v-model:trend-kind="trendKind"
    last-refreshed-at="刚刚更新"
    :metrics="snapshot.metrics"
    :trend-points="trend.points"
    :trend-summary="trend.summary"
    :health-timeline="snapshot.healthTimeline"
    :account-usage="snapshot.accountUsage"
    :wire-profiles="snapshot.wireProfiles"
    :usage-records="snapshot.usageRecords"
    :pool-summary="snapshot.poolSummary"
    :capacity-info="snapshot.capacityInfo"
    :rotation-strategy="snapshot.rotationStrategy"
  />
</template>
