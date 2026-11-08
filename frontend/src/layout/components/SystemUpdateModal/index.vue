<script setup lang="ts">
import type { SystemUpdateChannel } from '@/api'
import {
  ArrowUpCircle,
  Circle,
  ExternalLink,
  Power,
  RefreshCw,
  Terminal,
} from '@lucide/vue'
import { storeToRefs } from 'pinia'

import { computed, nextTick, useTemplateRef, watch } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseEmpty from '@/components/base/BaseEmpty.vue'
import BaseMarkdown from '@/components/base/BaseMarkdown/index.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseScrollbar from '@/components/base/BaseScrollbar.vue'
import { toast } from '@/components/base/BaseToast'
import { t } from '@/i18n'
import { useSystemUpdateStore } from '@/stores/modules/system-update'
import { errorMessage } from '@/utils/async'
import { formatTime } from '@/utils/date'
import {
  resolveSystemUpdateLogClasses,
  resolveSystemUpdatePresentation,
} from './presenter'

const open = defineModel<boolean>({ default: false })

const systemUpdateStore = useSystemUpdateStore()
const {
  version,
  updateInfo,
  loading,
  checking,
  updating,
  restarting,
  updateError,
  needRestart,
  updateLogs,
  updateStreaming,
  updateStreamError,
  hasCandidateUpdate: hasUpdate,
  canRestart,
  changingChannel,
  selectedChannel,
  availableChannels,
  canChangeChannel,
  canUpdate,
} = storeToRefs(systemUpdateStore)
const { loadSystem, checkUpdates, changeChannel, updateNow, restartNow } = systemUpdateStore

const updateLogScrollbar = useTemplateRef<InstanceType<typeof BaseScrollbar>>('updateLogScrollbar')
const presentation = computed(() => resolveSystemUpdatePresentation({
  version: version.value,
  updateInfo: updateInfo.value,
  loading: loading.value,
  restarting: restarting.value,
  updating: updating.value,
  updateError: updateError.value,
  updateSuccess: needRestart.value,
  hasUpdate: hasUpdate.value,
  updateStreaming: updateStreaming.value,
  updateStreamError: updateStreamError.value,

}))

const updateLogRows = computed(() =>
  updateLogs.value.map(item => ({
    ...item,
    time: formatTime(item.at, '--:--:--'),
    classes: resolveSystemUpdateLogClasses(item.level),
  })),
)

const hasReleaseNotes = computed(() => Boolean(updateInfo.value?.notes?.trim()))

const showUpdateProgress = computed(
  () => hasUpdate.value || updating.value || restarting.value || updateLogRows.value.length > 0,
)

async function scrollUpdateLogsToBottom() {
  await nextTick()
  await updateLogScrollbar.value?.scrollToBottom()
}

function pinUpdateLogsToBottom() {
  window.requestAnimationFrame(() => {
    void scrollUpdateLogsToBottom()
  })
}

async function handleCheckUpdates(force = true) {
  try {
    const data = await checkUpdates(force)
    if (!data || data.warning)
      return
    toast.success(data.hasUpdate ? '发现可用更新' : '当前已是最新版本')
  }
  catch {}
}

async function handleUpdateRequest() {
  const detail = updateInfo.value
  if (!canUpdate.value || !detail)
    return
  try {
    const result = await updateNow(detail.latestVersion, detail.policy.channel)
    if (result)
      toast.success('更新已开始')
  }
  catch {}
}

function handleChannelChange(event: Event) {
  const value = (event.target as HTMLSelectElement).value as SystemUpdateChannel
  void changeChannel(value).catch(() => undefined)
}

async function handleRestart() {
  if (!canRestart.value)
    return
  try {
    await restartNow()
  }
  catch (error: unknown) {
    toast.error(errorMessage(error, '重启失败'))
  }
}

watch(open, (visible) => {
  if (visible) {
    void loadSystem(false).catch(() => undefined)
  }
})

watch(
  () => updateLogs.value.at(-1)?.id,
  (logId, previousLogId) => {
    if (!logId || logId === previousLogId)
      return

    pinUpdateLogsToBottom()
  },
  { flush: 'post' },
)
</script>

<template>
  <BaseModal
    v-model="open"
    :title="t('systemUpdate.title')"
    :description="t('systemUpdate.description')"
    tone="success"
    size="lg"
    :dismissible="!restarting"
  >
    <template #icon>
      <ArrowUpCircle class="size-4.5 text-cp-success" />
    </template>

    <div class="grid gap-3.5">
      <section class="grid gap-4 rounded-cp-card bg-cp-fill-quaternary px-4 py-4">
        <div class="flex flex-wrap items-center justify-between gap-3">
          <div class="min-w-0">
            <p class="m-0 text-cp-xs leading-none font-heavy text-cp-text-quaternary">
              Codex Proxy RS
            </p>
            <p class="mt-2 mb-0 text-lg leading-none font-heavy text-cp-text">
              {{ t('systemUpdate.application') }}
            </p>
          </div>
          <span
            class="inline-flex h-7 shrink-0 items-center gap-1.5 rounded-full px-2.5 text-cp-sm font-heavy"
            :class="presentation.status.badge"
          >
            <component
              :is="presentation.status.icon"
              class="size-3.5"
              :class="presentation.status.iconClass"
            />
            {{ presentation.status.label }}
          </span>
        </div>

        <div class="grid gap-2.5 sm:grid-cols-4">
          <div
            v-for="item in presentation.summaryItems"
            :key="item.key"
            class="min-w-0 rounded-cp bg-cp-bg-container px-3 py-2.5"
          >
            <div class="flex min-w-0 items-center justify-between gap-2">
              <p class="m-0 truncate text-cp-xs leading-none font-heavy text-cp-text-quaternary">
                {{ item.label }}
              </p>
              <a
                v-if="item.releaseUrl"
                :href="item.releaseUrl"
                target="_blank"
                rel="noreferrer"
                class="inline-flex shrink-0 items-center gap-1 text-cp-xs leading-none font-bold text-cp-link transition-colors hover:text-cp-link-hover"
              >
                {{ t('systemUpdate.releasePage') }}
                <ExternalLink class="size-3" />
              </a>
            </div>
            <p
              class="mt-2 mb-0 truncate font-mono text-cp leading-none font-bold text-cp-text"
              :title="item.title || item.value"
            >
              {{ item.value }}
            </p>
          </div>
        </div>

        <p
          v-if="updateError || updateInfo?.warning"
          class="m-0 rounded-cp bg-cp-error-container px-3 py-2 text-cp-sm leading-normal font-bold text-cp-error-on-container"
        >
          {{ updateError || updateInfo?.warning }}
          <span v-if="updateInfo?.retryAt" class="mt-2 block">可重试时间：<time :datetime="updateInfo.retryAt">{{ new Date(updateInfo.retryAt).toLocaleString() }}</time></span>
        </p>
      </section>

      <section
        v-if="hasReleaseNotes"
        class="grid gap-2 rounded-cp-card bg-cp-fill-quaternary px-4 py-3.5"
      >
        <div class="flex items-center justify-between gap-3">
          <p class="m-0 text-cp font-heavy text-cp-text">
            {{ t('systemUpdate.releaseNotes') }}
          </p>
          <span class="font-mono text-cp-xs font-emphasis text-cp-text-quaternary">
            {{ presentation.releaseVersion }}
          </span>
        </div>
        <BaseScrollbar class="-mx-4" max-height="160px">
          <div class="px-4">
            <BaseMarkdown :source="updateInfo?.notes" />
          </div>
        </BaseScrollbar>
      </section>

      <section
        v-if="showUpdateProgress"
        class="overflow-hidden rounded-cp-card bg-cp-fill-quaternary"
      >
        <header class="flex items-center justify-between gap-3 px-4 pt-3.5 pb-2.5">
          <div class="flex min-w-0 items-center gap-2">
            <Terminal class="size-4 shrink-0 text-cp-success" />
            <p class="m-0 text-cp leading-none font-heavy text-cp-text">
              {{ t('systemUpdate.progress') }}
            </p>
          </div>
          <span
            class="inline-flex h-6 items-center gap-1.5 rounded-full bg-cp-fill-quaternary px-2 text-cp-xs leading-none font-bold text-cp-text-secondary"
            :title="updateStreamError || undefined"
          >
            <i
              class="size-1.5 rounded-full"
              :class="updateStreaming ? 'bg-cp-success' : 'bg-cp-text-quaternary'"
            />
            {{ presentation.streamStatusLabel }}
          </span>
        </header>

        <BaseScrollbar
          v-if="updateLogRows.length"
          ref="updateLogScrollbar"
          height="260px"
        >
          <div class="grid min-h-full gap-2 px-4 pb-4">
            <div
              v-for="log in updateLogRows"
              :key="log.id"
              class="grid grid-cols-[68px_14px_minmax(0,1fr)] items-start gap-2 rounded-cp bg-cp-bg-container px-3 py-2 font-mono text-cp-xs leading-[1.55]"
            >
              <span class="tabular-nums text-cp-text-quaternary">{{ log.time }}</span>
              <Circle
                class="mt-1 size-2.5"
                :class="log.classes.marker"
                fill="currentColor"
              />
              <p class="m-0 min-w-0 wrap-break-word" :class="log.classes.text">
                <span v-if="log.step" class="mr-1 text-cp-text-quaternary">[{{ log.step }}]</span>
                {{ log.message }}
              </p>
            </div>
          </div>
        </BaseScrollbar>
        <div v-else class="grid h-30 place-items-center px-4 pb-4">
          <BaseEmpty title="暂无进度" :icon="Terminal" size="sm" surface="none" />
        </div>
      </section>
    </div>

    <template #footer>
      <label for="system-update-channel" class="flex items-center gap-2 text-cp-sm">
        更新通道
        <select id="system-update-channel" :value="selectedChannel" :disabled="!canChangeChannel" aria-label="更新通道" class="rounded-cp bg-cp-bg-container px-2 py-1" @change="handleChannelChange">
          <option v-for="channel in availableChannels" :key="channel" :value="channel">{{ channel }}</option>
        </select>
      </label>
      <BaseButton
        variant="secondary"
        :loading="checking"
        :disabled="loading || updating || restarting || changingChannel"
        @click="handleCheckUpdates(true)"
      >
        <template #loading>
          <RefreshCw class="size-3.5 animate-spin motion-reduce:animate-none" />
        </template>
        <template #icon>
          <RefreshCw class="size-3.5" />
        </template>
        {{ t('systemUpdate.actions.check') }}
      </BaseButton>
      <BaseButton
        v-if="needRestart"
        variant="primary"
        :loading="restarting"
        :disabled="!canRestart"
        @click="handleRestart"
      >
        <template #icon>
          <Power class="size-4" />
        </template>
        {{ presentation.restartButtonLabel }}
      </BaseButton>
      <BaseButton
        v-else
        variant="primary"
        :loading="updating"
        :disabled="!canUpdate"
        @click="handleUpdateRequest"
      >
        <template #icon>
          <ArrowUpCircle class="size-4" />
        </template>
        立即更新
      </BaseButton>
    </template>
  </BaseModal>
</template>
