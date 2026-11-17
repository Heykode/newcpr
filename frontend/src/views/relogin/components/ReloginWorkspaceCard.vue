<script setup lang="ts">
import type { MaintenanceWorkspace } from '@/api/modules/relogin'
import AccountPlanBadge from '../../accounts/components/AccountPlanBadge.vue'
import { shortWorkspace } from '../presentation'

defineProps<{
  workspace?: MaintenanceWorkspace
  label?: string
  description?: string
  selected?: boolean
}>()
</script>

<template>
  <div class="flex min-w-0 items-center justify-between gap-3 rounded-cp-lg border p-3 text-left" :class="selected ? 'border-cp-primary bg-cp-fill-secondary' : 'border-cp-border-secondary'">
    <div class="min-w-0">
      <div class="flex flex-wrap items-center gap-2">
        <span class="break-words text-cp-sm font-semibold">{{ label || workspace?.name || '未命名工作区' }}</span>
        <AccountPlanBadge v-if="workspace?.planType" :plan-type="workspace.planType" :plan-type-display="workspace.planType.toUpperCase()" size="sm" />
      </div>
      <div v-if="workspace" class="mt-1 truncate font-mono text-cp-xs text-cp-text-tertiary" :title="workspace.id">
        {{ shortWorkspace(workspace.id) }}
      </div>
      <p v-if="description || workspace?.available === false" class="mb-0 mt-1 text-cp-xs text-cp-text-secondary">
        {{ workspace?.available === false ? '不可访问' : description }}
      </p>
    </div>
    <slot />
  </div>
</template>
