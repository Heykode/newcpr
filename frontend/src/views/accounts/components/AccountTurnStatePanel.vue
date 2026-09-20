<script setup lang="ts">
import type { AccountRow } from '../constants'
import { ShieldCheck } from '@lucide/vue'
import { computed } from 'vue'

import { useUiClock } from '@/composables/useUiClock'
import { turnStateBlockReason, turnStateProbeReason } from '../utils/turnState'

const props = defineProps<{
  account: AccountRow
}>()

const now = useUiClock()
const blockedReason = computed(() => turnStateBlockReason(props.account))
const enabled = computed(() => props.account.turnStateInjectionEnabled && props.account.turnState?.enabled !== false)

interface StateSlot {
  chars: number | null
  capturedAt?: string | null
  expiresAt: string
}

interface StateModel {
  model: string
  refreshStatus: string
  probeAttempts?: number
  probeTotalAttempts?: number
  probeCooldownUntil?: string | null
  probeRetryFromUpstream?: boolean | null
  probeHttpStatus?: number | null
  probeErrorCode?: string | null
  probeReturnedLength?: number | null
  successfulProbeAttempt?: number | null
  lastProbeReason?: string | null
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
  const ready = Boolean(active && validTimestamp(active.expiresAt)! > now.value.getTime() + 60_000)
  const cooldownUntil = model.probeCooldownUntil ? validTimestamp(model.probeCooldownUntil) : null
  const cooldownSeconds = cooldownUntil === null ? 0 : Math.max(0, Math.ceil((cooldownUntil - now.value.getTime()) / 1000))
  let status = '待采集'
  let statusClass = 'text-cp-warning-text'
  if (!enabled.value) {
    status = '缓存保留'
    statusClass = 'text-cp-text-quaternary'
  }
  else if (blockedReason.value) {
    status = '已停止'
    statusClass = 'text-cp-error-text'
  }
  else if (cooldownSeconds > 0) {
    status = ready ? '可用 · 探测冷却' : '探测冷却'
    statusClass = ready ? 'text-cp-success-text' : 'text-cp-warning-text'
  }
  else if (ready && standby) {
    status = '等待切换'
    statusClass = 'text-cp-success-text'
  }
  else if (ready) {
    status = model.refreshStatus === 'refreshing' ? '刷新中' : model.refreshStatus === 'queued' ? '刷新排队' : '已就绪'
    statusClass = 'text-cp-success-text'
  }
  else if (model.refreshStatus === 'refreshing') {
    status = '采集中'
    statusClass = 'text-cp-warning-text'
  }
  else if (model.refreshStatus === 'queued') {
    status = '排队中'
  }
  else if (model.refreshStatus === 'cooldown') {
    status = '等待重试'
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
    ready: ready && enabled.value && !blockedReason.value,
    status,
    statusClass,
    cooldownSeconds,
  }
}))

const readyCount = computed(() => rows.value.filter(row => row.ready).length)
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
  return `${label}：${slot.chars ?? '未知'} 字符；采集时间 ${captureTime(slot, true)}；到期时间 ${absolute} (UTC+8)`
}

function captureTime(slot: StateSlot | null, full = false) {
  const timestamp = slot?.capturedAt ? validTimestamp(slot.capturedAt) : null
  if (timestamp === null)
    return '未知'
  const date = new Date(timestamp)
  return full
    ? date.toLocaleString('zh-CN', { hour12: false, timeZone: 'Asia/Shanghai' })
    : date.toLocaleTimeString('zh-CN', { hour12: false, timeZone: 'Asia/Shanghai' })
}

function probeTitle(row: StateModel) {
  const details = [
    slotTitle('当前', row.active),
    `本轮尝试 ${row.probeAttempts ?? 0} 次`,
    `累计 ${row.probeTotalAttempts ?? row.probeAttempts ?? 0} 次（历史轮次可能未记录）`,
  ]
  if (row.successfulProbeAttempt)
    details.push(`第 ${row.successfulProbeAttempt} 次尝试采集成功`)
  if (row.lastProbeReason)
    details.push(turnStateProbeReason(row.lastProbeReason) ?? row.lastProbeReason)
  if (row.probeReturnedLength != null)
    details.push(`最近返回 ${row.probeReturnedLength} 字符`)
  if (row.probeHttpStatus != null)
    details.push(`最近限流 HTTP ${row.probeHttpStatus}`)
  if (row.probeErrorCode)
    details.push(`限流错误码 ${row.probeErrorCode}`)
  if (row.probeCooldownUntil) {
    details.push(`探测恢复时间 ${row.probeCooldownUntil}`)
    details.push(row.probeRetryFromUpstream ? '等待时间来源：上游' : '等待时间来源：后台设置')
  }
  return details.join('；')
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
        </div>
      </div>
      <p
        v-if="rows.length"
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
      v-else-if="account.turnState?.enabled === false"
      class="m-0 mt-3 text-cp-xs font-emphasis text-cp-text-quaternary"
    >
      总开关已关闭
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
      v-if="rows.length"
      data-account-turn-state-list
      role="region"
      aria-label="模型 State 状态"
      :tabindex="rows.length > 3 ? 0 : undefined"
      class="mt-2 max-h-60 overflow-y-auto overscroll-contain pr-2 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-cp-border-secondary max-sm:max-h-72"
    >
      <ul class="m-0 list-none p-0">
        <li
          v-for="row in rows"
          :key="row.model"
          data-account-turn-state-model
          class="h-20 border-t border-cp-border-secondary py-2 first:border-t-0 max-sm:h-24"
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
                { key: 'active', label: '当前', value: row.active },
                ...(row.standby ? [{ key: 'standby', label: '待切换', value: row.standby }] : []),
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
          <div
            class="mt-1 truncate text-[10px] leading-3.5 text-cp-text-quaternary"
            :title="probeTitle(row)"
          >
            <span v-if="enabled && row.cooldownSeconds > 0 && !blockedReason">{{ row.cooldownSeconds }}s 后重试 · </span>
            本轮 {{ row.probeAttempts ?? 0 }} 次
            · 累计 {{ row.probeTotalAttempts ?? row.probeAttempts ?? 0 }} 次
            <span v-if="row.successfulProbeAttempt"> · 第 {{ row.successfulProbeAttempt }} 次成功</span>
            <span v-if="row.active"> · 采集 {{ captureTime(row.active) }}</span>
            <span v-if="row.lastProbeReason"> · {{ turnStateProbeReason(row.lastProbeReason) }}</span>
          </div>
        </li>
      </ul>
    </div>
  </section>
</template>
