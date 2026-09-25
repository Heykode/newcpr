<script setup lang="ts">
import type { AccountGroup } from '@/api'
import type { TokenGuardConfig, TokenGuardEvent, TokenGuardStatus } from '@/api/modules/token-guard'
import { Play, RefreshCw, Save } from '@lucide/vue'
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { getAccountGroups } from '@/api'
import { configureTokenGuard, getTokenGuard, reloginTokenGuardAccount, runTokenGuard } from '@/api/modules/token-guard'
import AccountGroupCheckboxGrid from '@/components/AccountGroupCheckboxGrid.vue'
import BaseButton from '@/components/base/BaseButton.vue'
import FormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseNumberInput from '@/components/base/BaseNumberInput.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { formatDateTime } from '@/utils/date'

const status = ref<TokenGuardStatus | null>(null)
const form = ref<TokenGuardConfig | null>(null)
const groups = ref<AccountGroup[]>([])
const saveAction = useAsyncAction()
const runAction = useAsyncAction()
const loginAction = useAsyncAction()
const loading = ref(false)
const error = ref(false)
const dirty = computed(() => form.value && status.value && JSON.stringify(form.value) !== JSON.stringify(status.value.config))
let active = true
let timer: ReturnType<typeof setTimeout> | undefined
let controller: AbortController | undefined
let generation = 0
const groupsController = new AbortController()

const reasons: Record<TokenGuardEvent['reason'], string> = {
  completed: '凭证可用',
  credential_expired: '凭证失效，需重登',
  credential_invalid: '配置无效，需重新导入',
  account_banned: '账号封禁，已跳过',
  account_disabled: '账号已暂停',
  quota_exhausted: '额度耗尽，已跳过',
  account_changed: '账号已更新，旧结果不生效',
  request_failed: '临时异常或模型不可用',
  timeout: '探活超时',
}

async function load() {
  if (!active || loading.value)
    return
  loading.value = true
  const currentGeneration = generation
  controller = new AbortController()
  try {
    const result = await getTokenGuard({ silent: true, signal: controller.signal })
    if (!active || currentGeneration !== generation)
      return
    if (!form.value)
      form.value = structuredClone(result.config)
    status.value = result
    error.value = false
  }
  catch {
    if (active)
      error.value = true
  }
  finally {
    loading.value = false
  }
}

async function poll() {
  await load()
  if (active)
    timer = setTimeout(poll, 5000)
}

async function save() {
  if (!form.value)
    return
  const submitted = { ...form.value, groupIds: [...form.value.groupIds] }
  await saveAction.run(async () => {
    await configureTokenGuard(submitted)
    generation++
    controller?.abort()
    if (active && status.value)
      status.value.config = submitted
    toast.success('凭证守护设置已保存')
    await load()
  })
}

async function run() {
  await runAction.run(async () => {
    await runTokenGuard()
    await load()
  })
}

async function relogin(accountId: string) {
  await loginAction.run(async () => {
    await reloginTokenGuardAccount(accountId)
    toast.success('已进入失效重登队列')
  })
}

onMounted(async () => {
  void poll()
  const result: AccountGroup[] = []
  try {
    for (let page = 1, pageCount = 1; page <= pageCount; page++) {
      if (!active)
        break
      const response = await getAccountGroups({ page, pageSize: 100 }, { silent: true, signal: groupsController.signal })
      result.push(...response.items)
      pageCount = response.page.totalPages
    }
    if (active)
      groups.value = result
  }
  catch {}
})
onBeforeUnmount(() => {
  active = false
  clearTimeout(timer)
  controller?.abort()
  groupsController.abort()
})
</script>

<template>
  <div class="flex min-w-0 flex-col gap-6">
    <BasePageHeader title="凭证守护">
      <template #actions>
        <BaseIconButton label="刷新" :disabled="loading" @click="load">
          <RefreshCw class="size-4" />
        </BaseIconButton>
        <BaseButton :disabled="!status?.config.enabled || status.running || status.queued || runAction.loading.value" @click="run">
          <Play class="size-4" />立即巡检
        </BaseButton>
      </template>
    </BasePageHeader>
    <p v-if="error" role="alert" class="text-cp-error">
      读取失败，请重试。
    </p>
    <form v-if="form" class="border-y border-cp-border py-5" @submit.prevent="save">
      <div class="mb-5 flex flex-wrap items-center justify-between gap-3">
        <BaseSwitch v-model="form.enabled" label="启用凭证守护" show-label />
        <span class="text-cp-text-secondary">{{ status?.running ? '巡检中' : status?.queued ? '等待巡检' : status?.config.enabled ? '等待下轮巡检' : '已关闭' }}</span>
      </div>
      <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        <FormItem label="探活模型">
          <BaseInput v-model="form.model" />
        </FormItem>
        <div class="grid gap-2 text-cp-sm">
          <span>巡检间隔</span><BaseNumberInput v-model="form.intervalSeconds" label="巡检间隔" unit="秒" :min="30" :max="86400" />
        </div>
        <div class="grid gap-2 text-cp-sm">
          <span>单请求超时</span><BaseNumberInput v-model="form.timeoutSeconds" label="单请求超时" unit="秒" :min="5" :max="900" />
        </div>
        <div class="grid gap-2 text-cp-sm">
          <span>探活并发</span><BaseNumberInput v-model="form.concurrency" label="探活并发" :min="1" :max="16" />
        </div>
        <div class="grid gap-2 text-cp-sm">
          <span>每轮账号数</span><BaseNumberInput v-model="form.maxPerCycle" label="每轮账号数" :min="1" :max="100" />
        </div>
      </div>
      <fieldset class="mt-5 min-w-0 border-0 p-0">
        <legend class="mb-2 text-cp-sm">
          守护分组（未选为全部）
        </legend>
        <AccountGroupCheckboxGrid v-model="form.groupIds" :groups="groups" />
      </fieldset>
      <div class="mt-5 flex flex-wrap items-center justify-between gap-3">
        <RouterLink to="/relogin" class="text-cp-primary-text">
          失效重登设置
        </RouterLink>
        <BaseButton type="submit" :disabled="!dirty || saveAction.loading.value">
          <Save class="size-4" />保存
        </BaseButton>
      </div>
    </form>
    <section class="min-w-0">
      <h2 class="mb-3 text-lg font-semibold">
        最近巡检记录
      </h2>
      <div class="overflow-x-auto">
        <table class="w-full border-collapse text-left text-cp-sm">
          <thead class="text-cp-text-secondary">
            <tr>
              <th class="p-3">
                账号
              </th><th class="p-3">
                结果
              </th><th class="p-3">
                耗时
              </th><th class="p-3">
                时间
              </th><th class="p-3">
                操作
              </th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="(event, index) in status?.events ?? []" :key="`${event.accountId}-${event.observedAt}-${index}`" class="border-t border-cp-border">
              <td class="max-w-60 truncate p-3 font-mono" :title="event.accountId">
                {{ event.accountId }}
              </td>
              <td class="p-3" :class="event.outcome === 'healthy' ? 'text-cp-success' : event.outcome === 'auth_required' ? 'text-cp-error' : 'text-cp-text-secondary'">
                {{ reasons[event.reason] }}
              </td>
              <td class="whitespace-nowrap p-3 tabular-nums">
                {{ event.latencyMs }} ms
              </td>
              <td class="whitespace-nowrap p-3">
                {{ formatDateTime(event.observedAt) }}
              </td>
              <td class="p-3">
                <BaseIconButton v-if="event.outcome === 'auth_required'" label="重新登录" :disabled="loginAction.loading.value" @click="relogin(event.accountId)">
                  <RefreshCw class="size-4" />
                </BaseIconButton>
              </td>
            </tr>
            <tr v-if="!status?.events.length">
              <td colspan="5" class="p-8 text-center text-cp-text-secondary">
                {{ loading ? '加载中...' : '暂无巡检记录' }}
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>
  </div>
</template>
