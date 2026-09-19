<script setup lang="ts">
import type { AccountRow } from '../constants'
import { ShieldCheck } from '@lucide/vue'
import { computed } from 'vue'

import { useUiClock } from '@/composables/useUiClock'
import { turnStateBlockReason } from '../utils/turnState'

const props = defineProps<{
  account: AccountRow
}>()

const now = useUiClock()
const blockedReason = computed(() => turnStateBlockReason(props.account))

interface StateSlot {
  chars: number | null
  expiresAt: string
}

interface StateModel {
  model: string
  refreshStatus: string
  active: StateSlot | null
  standby: StateSlot | null
}

function validTimestamp(value: string) {
  const timestamp = Date.parse(value)
  return Number.isFinite(timestamp) ? timestamp : null
}

function activeSlot(slot: StateSlot | null) {
  const expiresAt = slot ? validTimestamp(slot.expiresAt) : null
  return slot && expiresAt !== null && expiresAt > now.value.getTime() ? slot : null
}

const models = computed<StateModel[]>(() => {
  const turnState = props.account.turnState
  if (!turnState)
    return []
  if (turnState.models?.length)
    return turnState.models

  const ready = new Map(turnState.readyModels.map(item => [item.model, item.expiresAt]))
  return turnState.requiredModels.map(model => ({
    model,
    refreshStatus: ready.has(model) ? 'ready' : 'missing',
    active: ready.has(model) ? { chars: null, expiresAt: ready.get(model)! } : null,
    standby: null,
  }))
})

const rows = computed(() => models.value.map((model) => {
  const active = activeSlot(model.active)
  const standby = activeSlot(model.standby)
  let status = '待采集'
  let statusClass = 'text-cp-warning-text'
  if (active && standby) {
    status = '主备就绪'
    statusClass = 'text-cp-success-text'
  }
  else if (active) {
    status = model.refreshStatus === 'refreshing' ? '补充备用' : '已就绪'
    statusClass = 'text-cp-success-text'
  }
  else if (model.refreshStatus === 'refreshing') {
    status = '采集中'
    statusClass = 'text-cp-warning-text'
  }
  else if (model.refreshStatus === 'failed') {
    status = '等待重试'
    statusClass = 'text-cp-error-text'
  }

  return {
    ...model,
    active,
    standby,
    captured: Number(Boolean(active)) + Number(Boolean(standby)),
    status,
    statusClass,
  }
}))

const readyCount = computed(() => rows.value.filter(row => row.active).length)
const capturedCount = computed(() => rows.value.reduce((total, row) => total + row.captured, 0))

function slotCountdown(slot: StateSlot | null) {
  if (!slot)
    return '未获取'
  const expiresAt = validTimestamp(slot.expiresAt)
  if (expiresAt === null)
    return '时间未知'
  const remaining = expiresAt - now.value.getTime()
  if (remaining <= 0)
    return '已过期'
  const minutes = Math.floor(remaining / 60_000)
  if (minutes >= 24 * 60)
    return `${Math.floor(minutes / (24 * 60))}d ${Math.floor(minutes / 60) % 24}h`
  if (minutes >= 60)
    return `${Math.floor(minutes / 60)}h ${minutes % 60}m`
  return minutes > 0 ? `${minutes}m` : '<1m'
}

function slotTitle(label: string, slot: StateSlot | null) {
  if (!slot)
    return `${label} 尚未获取`
  const expiresAt = validTimestamp(slot.expiresAt)
  const absolute = expiresAt === null
    ? slot.expiresAt
    : new Date(expiresAt).toLocaleString('zh-CN', { hour12: false, timeZone: 'Asia/Shanghai' })
  return `${label}：${slot.chars ?? '未知'} 字符；到期时间 ${absolute} (UTC+8)`
}
</script>

<template>
  <section
    v-if="account.provider === 'openai'"
    data-account-turn-state-panel
    class="mt-auto border-t border-cp-border-secondary pt-3 max-sm:w-[calc(100vw-5.5rem)]"
  >
    <div class="flex min-w-0 flex-wrap items-start justify-between gap-3">
      <div class="flex min-w-0 items-center gap-2">
        <ShieldCheck class="size-4 shrink-0 text-cp-text-tertiary" :stroke-width="1.75" />
        <div class="min-w-0">
          <h4 class="m-0 text-cp-sm font-heavy text-cp-text">
            State 注入
          </h4>
          <p class="m-0 mt-0.5 text-[10px] leading-3.5 font-emphasis text-cp-text-quaternary">
            按账号与模型独立维护
          </p>
        </div>
      </div>
      <p
        v-if="account.turnStateInjectionEnabled && !blockedReason && rows.length"
        class="m-0 shrink-0 text-right text-cp-xs font-emphasis text-cp-text-secondary max-sm:w-full max-sm:text-left"
      >
        <strong class="font-mono font-heavy tabular-nums text-cp-text">{{ readyCount }}/{{ rows.length }}</strong> 模型
        <span class="mx-1 text-cp-text-quaternary">·</span>
        <strong class="font-mono font-heavy tabular-nums text-cp-text">{{ capturedCount }}</strong> 个 State
      </p>
    </div>

    <p
      v-if="!account.turnStateInjectionEnabled"
      class="m-0 mt-3 text-cp-xs font-emphasis text-cp-text-quaternary"
    >
      账号级开关已关闭
    </p>
    <p
      v-else-if="blockedReason"
      data-account-turn-state-blocked
      class="m-0 mt-3 text-cp-xs font-emphasis text-cp-error-text"
    >
      {{ blockedReason }}
    </p>
    <p
      v-else-if="!account.turnState"
      class="m-0 mt-3 text-cp-xs font-emphasis text-cp-warning-text"
    >
      全局未启用、账号已暂停或状态暂不可用
    </p>
    <p
      v-else-if="rows.length === 0"
      class="m-0 mt-3 text-cp-xs font-emphasis text-cp-text-quaternary"
    >
      当前没有需要维护的模型
    </p>

    <div
      v-else
      data-account-turn-state-list
      role="region"
      aria-label="模型 State 状态"
      :tabindex="rows.length > 3 ? 0 : undefined"
      class="mt-2 max-h-[10.5rem] overflow-y-auto overscroll-contain pr-2 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-cp-border-secondary max-sm:max-h-[14.25rem]"
    >
      <ul class="m-0 list-none p-0">
        <li
          v-for="row in rows"
          :key="row.model"
          data-account-turn-state-model
          class="h-14 border-t border-cp-border-secondary py-2 first:border-t-0 max-sm:h-[4.75rem]"
        >
          <div class="flex min-w-0 items-center justify-between gap-3">
            <span class="min-w-0 truncate font-mono text-cp-xs font-heavy text-cp-text" :title="row.model">
              {{ row.model }}
            </span>
            <span class="shrink-0 text-[10px] leading-3.5 font-heavy" :class="row.statusClass">
              {{ row.status }}
            </span>
          </div>
          <div class="mt-1.5 grid min-w-0 grid-cols-2 gap-x-3 gap-y-1 text-[10px] leading-3.5 max-sm:grid-cols-1">
            <div
              v-for="slot in [
                { key: 'active', label: 'Active', value: row.active },
                { key: 'standby', label: 'Standby', value: row.standby },
              ]"
              :key="slot.key"
              class="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] gap-x-1.5"
              :title="slotTitle(slot.label, slot.value)"
            >
              <span class="font-bold text-cp-text-quaternary">{{ slot.label }}</span>
              <span class="min-w-0 truncate text-right font-mono font-emphasis tabular-nums" :class="slot.value ? 'text-cp-text-secondary' : 'text-cp-text-quaternary'">
                <template v-if="slot.value">
                  {{ slot.value.chars ?? '—' }} 字符 · {{ slotCountdown(slot.value) }}
                </template>
                <template v-else>
                  未获取
                </template>
              </span>
            </div>
          </div>
        </li>
      </ul>
    </div>
  </section>
</template>
