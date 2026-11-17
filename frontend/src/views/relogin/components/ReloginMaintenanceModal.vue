<script setup lang="ts">
import type { MaintenanceAction, MaintenanceView, ReloginEntry } from '@/api/modules/relogin'
import { computed, shallowRef, watch } from 'vue'
import { getRelogin, maintainRelogin } from '@/api/modules/relogin'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseSegmented from '@/components/base/BaseSegmented.vue'
import { errorMessage } from '@/utils/async'
import { formatDateTime } from '@/utils/date'
import { maintenanceBusy, maintenanceLabels, sessionLabel } from '../maintenance'
import ReloginEgress from './ReloginEgress.vue'
import ReloginWorkspaceCard from './ReloginWorkspaceCard.vue'

const props = defineProps<{ entry: ReloginEntry | null, initialTab?: string }>()
const emit = defineEmits<{ changed: [], afterLeave: [], credentials: [] }>()
const open = defineModel<boolean>({ default: false })
const tab = shallowRef('session')
const tabs = [{ label: '登录与授权', value: 'session' }, { label: '设备', value: 'devices' }, { label: '工作区', value: 'workspaces' }, { label: '安全维护', value: 'security' }]
const current = shallowRef<ReloginEntry | null>(null)
const result = shallowRef<MaintenanceView | undefined>()
const busy = shallowRef(false)
const failure = shallowRef('')
const confirmation = shallowRef(false)
const pending = shallowRef<{ action: MaintenanceAction, target?: string, deviceIdHash?: string, revision: number } | null>(null)
const view = computed(() => result.value ?? current.value?.maintenance)
const taskBusy = computed(() => busy.value || (!!current.value && maintenanceBusy(current.value)))
let generation = 0
watch([() => open.value, () => props.entry?.id, () => props.initialTab], ([visible]) => {
  generation++
  failure.value = ''
  if (visible) {
    current.value = props.entry
    result.value = undefined
    tab.value = props.initialTab ?? 'session'
  }
  else {
    confirmation.value = false
  }
}, { flush: 'sync' })
watch(() => props.entry, (value) => {
  if (open.value && value?.id === current.value?.id && !busy.value) {
    current.value = value
    result.value = undefined
  }
})
function ask(action: MaintenanceAction, target?: string, deviceIdHash?: string) {
  if (!current.value || taskBusy.value)
    return
  pending.value = { action, target, deviceIdHash, revision: current.value.revision }
  confirmation.value = true
}
function showCredentials() {
  open.value = false
  emit('credentials')
}
async function run(action: MaintenanceAction, target?: string, deviceIdHash?: string, confirmed = false, revision = current.value?.revision) {
  if (!current.value || taskBusy.value)
    return
  const owner = generation
  const id = current.value.id
  busy.value = true
  failure.value = ''
  try {
    const outcome = await maintainRelogin({ id, revision: revision!, action, target, deviceIdHash, ...(confirmed ? { confirm: 'maintain_account' } : {}) })
    emit('changed')
    if (owner === generation) {
      result.value = outcome
      confirmation.value = false
      const rows = await getRelogin({ silent: true })
      if (owner === generation) {
        current.value = rows.items.find(row => row.id === id) ?? null
        result.value = undefined
      }
    }
  }
  catch (error) {
    if (owner === generation)
      failure.value = errorMessage(error, '维护失败')
    confirmation.value = false
    emit('changed')
  }
  finally { busy.value = false }
}
function afterLeave() {
  if (!open.value) {
    current.value = null
    result.value = undefined
    pending.value = null
  }
  emit('afterLeave')
}
</script>

<template>
  <BaseModal v-model="open" title="账号维护" :description="current?.displayName || current?.email" size="lg" @after-leave="afterLeave">
    <div v-if="current" class="mb-4 flex min-w-0 items-center gap-3 rounded-cp-lg bg-cp-fill-quaternary px-3 py-2">
      <span class="shrink-0 text-cp-xs text-cp-text-tertiary">当前出口</span><ReloginEgress class="min-w-0 flex-1" :row="current" />
    </div>
    <BaseSegmented v-model="tab" label="维护项目" :options="tabs" class="mb-5 w-full" />
    <p v-if="failure" role="alert" class="break-words text-cp-error">
      {{ failure }}
    </p>
    <div v-if="view?.message" class="mb-4 rounded-cp border border-cp-border-secondary p-3 text-cp-sm" :class="['failed', 'uncertain'].includes(view.state) ? 'text-cp-warning' : 'text-cp-text-secondary'" role="status">
      {{ view.message }}
    </div>
    <div v-if="tab === 'session'" class="grid gap-4">
      <div class="flex items-center justify-between border-b border-cp-border-secondary pb-3">
        <div>
          <div class="font-semibold">
            Web 登录会话
          </div><span class="text-cp-xs text-cp-text-secondary">{{ current ? sessionLabel({ ...current, maintenance: view }) : '未检测' }} · {{ view?.sessionCheckedAt ? formatDateTime(view.sessionCheckedAt) : '尚未核对' }}</span>
        </div>
        <BaseButton size="sm" :loading="busy" :disabled="taskBusy" @click="run('check_session')">
          检查 Web 会话
        </BaseButton>
      </div>
      <p class="m-0 text-cp-xs text-cp-text-tertiary">
        会话检查只确认网页登录是否有效，不会获取新的号池凭据。
      </p>
      <div class="maintenance-choice">
        <div><strong>获取新的 Codex 凭据</strong><p>可复用有效会话，排队获取新凭据</p></div><BaseButton :disabled="taskBusy" variant="primary" @click="run('authorize')">
          获取新凭据
        </BaseButton>
      </div>
      <div class="maintenance-choice">
        <div><strong>密码与 2FA 重登</strong><p>排队重新登录，建立新会话</p></div><BaseButton :disabled="taskBusy" @click="run('force_login')">
          密码与 2FA 重登
        </BaseButton>
      </div>
      <div class="maintenance-choice">
        <div><strong>退出并重登</strong><p>先提交全部退出，成功后进入重登队列</p></div><BaseButton :disabled="taskBusy" @click="ask('logout_and_relogin')">
          退出并重登
        </BaseButton>
      </div>
      <p v-if="view?.loginPaused" class="m-0 text-cp-xs text-cp-warning">
        已退出全部设备，自动重登暂停；主动重新授权后恢复
      </p>
      <p class="m-0 text-cp-xs text-cp-text-tertiary">
        获取的凭据仍需确认推送到号池
      </p>
    </div>
    <div v-else-if="tab === 'devices'" class="grid gap-3">
      <div class="flex flex-wrap justify-between gap-2">
        <BaseButton :loading="busy" :disabled="taskBusy" @click="run('devices')">
          刷新设备
        </BaseButton><BaseButton :disabled="taskBusy" @click="ask('logout_all')">
          退出全部设备
        </BaseButton>
      </div>
      <div v-for="device in view?.devices ?? []" :key="device.id" class="maintenance-choice">
        <div class="min-w-0">
          <strong class="break-words">{{ device.name }}</strong><span v-if="device.current" class="ml-2 text-cp-xs text-cp-primary">当前设备</span><p>{{ device.platform || '平台未知' }} · {{ device.lastActiveAt ? formatDateTime(new Date(device.lastActiveAt * 1000).toISOString()) : '活动时间未知' }}</p>
        </div>
        <BaseButton size="sm" :disabled="taskBusy || device.current" @click="ask('revoke_device', device.sessionId ?? undefined, device.sessionId ? undefined : device.deviceIdHash ?? undefined)">
          退出
        </BaseButton>
      </div>
      <p v-if="!view?.devices?.length" class="text-cp-sm text-cp-text-secondary">
        点击刷新读取上游设备列表
      </p>
    </div>
    <div v-else-if="tab === 'workspaces'" class="grid gap-3">
      <div>
        <BaseButton :loading="busy" :disabled="taskBusy" @click="run('workspaces')">
          刷新工作区
        </BaseButton>
      </div>
      <ReloginWorkspaceCard v-for="workspace in view?.workspaces ?? []" :key="workspace.id" :workspace="workspace">
        <BaseButton size="sm" :disabled="taskBusy || !workspace.available" @click="ask('switch_workspace', workspace.id)">
          选择并授权
        </BaseButton>
      </ReloginWorkspaceCard>
      <p class="m-0 text-cp-xs text-cp-text-secondary">
        切换后重新获取凭据，推送时再确认号池目标
      </p>
    </div>
    <div v-else class="grid gap-4">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <span class="text-cp-sm">2FA {{ view?.totpEnabled == null ? '未核对' : view.totpEnabled ? '已启用' : '未启用' }} · 密码 {{ view?.passwordEnabled == null ? '未核对' : view.passwordEnabled ? '已设置' : '未设置' }}</span><BaseButton :loading="busy" :disabled="taskBusy" @click="run('security')">
          核对安全状态
        </BaseButton>
      </div>
      <div class="maintenance-choice">
        <div><strong>2FA 密钥</strong><p>{{ view?.pendingTotp ? '候选密钥已保存，待激活' : '轮换按停用、准备、激活分步确认' }}</p></div><BaseButton :disabled="busy" @click="showCredentials">
          查看资料
        </BaseButton>
      </div>
      <div class="flex flex-wrap gap-2">
        <BaseButton :disabled="taskBusy || view?.uncertain || view?.totpEnabled !== true" @click="ask('disable_totp')">
          停用旧 2FA
        </BaseButton>
        <BaseButton :disabled="taskBusy || view?.uncertain || view?.totpEnabled !== false || view?.pendingTotp" @click="ask('prepare_totp')">
          准备新 2FA
        </BaseButton>
        <BaseButton variant="primary" :disabled="taskBusy || view?.uncertain || !view?.pendingTotp" @click="ask('activate_totp')">
          激活新 2FA
        </BaseButton>
      </div>
      <p class="m-0 text-cp-xs text-cp-text-secondary">
        远端改密需要官方安全验证，完成后在资料弹窗更新本地密码
      </p>
    </div>
    <BaseConfirmModal v-model="confirmation" :title="pending ? maintenanceLabels[pending.action] : '确认维护'" :loading="busy" @confirm="pending && run(pending.action, pending.target, pending.deviceIdHash, true, pending.revision)">
      <p v-if="pending?.action === 'switch_workspace'">
        将排队为所选工作区获取凭据，完成后待确认推送
      </p>
      <p v-else-if="pending?.action === 'disable_totp'">
        此操作会关闭上游当前 2FA，完成后需继续准备并激活新密钥
      </p>
      <p v-else-if="pending?.action === 'prepare_totp'">
        将申请并保存新的候选密钥，尚不替换当前资料
      </p>
      <p v-else-if="pending?.action === 'activate_totp'">
        将激活已保存的候选密钥，成功后更新本地 2FA 资料
      </p>
      <p v-else>
        将向上游提交退出请求，相关设备可能需要重新登录，上游生效可能存在延迟
      </p>
    </BaseConfirmModal>
  </BaseModal>
</template>

<style scoped>
.maintenance-choice {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  border: 1px solid var(--cp-border-secondary);
  border-radius: var(--cp-radius, 8px);
  padding: 16px;
}
.maintenance-choice p {
  margin: 6px 0 0;
  color: var(--cp-text-secondary);
  font-size: 12px;
}
@media (max-width: 480px) {
  .maintenance-choice {
    align-items: flex-start;
    flex-direction: column;
    gap: 12px;
  }
}
</style>
