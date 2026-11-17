<script setup lang="ts">
import type { ReloginEntry } from '@/api/modules/relogin'
import { sessionLabel } from '../maintenance'
import { credentialLabel, poolPresentation } from '../presentation'

defineProps<{ row: ReloginEntry, compact?: boolean }>()
</script>

<template>
  <div class="grid text-cp-xs" :class="compact ? 'grid-cols-3 gap-2' : 'gap-1.5'">
    <div class="flex min-w-0 items-center gap-1" :class="{ 'flex-wrap content-start rounded-cp bg-cp-fill-quaternary p-2': compact }">
      <span class="size-1.5 shrink-0 rounded-full" :class="row.maintenance?.sessionState === 'valid' ? 'bg-cp-success' : row.maintenance?.sessionState === 'expired' ? 'bg-cp-warning' : 'bg-cp-text-tertiary'" />
      <span class="w-8 shrink-0 text-cp-text-tertiary">Web</span><span class="min-w-0 whitespace-normal break-words" :class="{ 'basis-full': compact }">{{ sessionLabel(row) }}</span>
    </div>
    <div class="flex min-w-0 items-center gap-1" :class="{ 'flex-wrap content-start rounded-cp bg-cp-fill-quaternary p-2': compact }" title="本次获取的 Codex 凭据与推送状态；与 Web 会话分别显示">
      <span class="size-1.5 shrink-0 rounded-full" :class="row.poolStatus === 'synced' ? 'bg-cp-success' : row.credentialStatus === 'verified' || row.credentialStatus === 'expired' ? 'bg-cp-warning' : 'bg-cp-text-tertiary'" />
      <span class="w-8 shrink-0 text-cp-text-tertiary">Codex</span><span class="min-w-0 whitespace-normal break-words" :class="{ 'basis-full': compact }">{{ row.poolStatus === 'synced' ? '已同步' : row.credentialStatus === 'verified' ? '待推送' : credentialLabel(row) }}</span>
    </div>
    <div class="flex min-w-0 items-center gap-1" :class="{ 'flex-wrap content-start rounded-cp bg-cp-fill-quaternary p-2': compact }" :title="poolPresentation(row).detail">
      <span class="size-1.5 shrink-0 rounded-full bg-current" :class="poolPresentation(row).tone" />
      <span class="w-8 shrink-0 text-cp-text-tertiary">号池</span><span class="min-w-0 whitespace-normal break-words" :class="[poolPresentation(row).tone, { 'basis-full': compact }]">{{ poolPresentation(row).label }}</span>
    </div>
  </div>
</template>
