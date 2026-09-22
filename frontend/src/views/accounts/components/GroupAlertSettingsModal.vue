<script setup lang="ts">
import type { AccountGroup, BarkLevel, GroupAlertPolicy, GroupMonitorItem, NotificationChannels } from '@/api'
import { BellRing, ChevronDown, Mail, Send } from '@lucide/vue'
import { computed, reactive, ref, watch } from 'vue'
import { getGroupAlertPolicy, getNotificationChannels, testNotification, updateGroupAlertPolicy } from '@/api'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseNumberInput from '@/components/base/BaseNumberInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import { toast } from '@/components/base/BaseToast'
import { errorMessage } from '@/utils/async'
import { monitorMoney } from './group-monitor-presentation'

const props = defineProps<{ group: AccountGroup | null, snapshot?: GroupMonitorItem, stale?: boolean, now?: number }>()
const open = defineModel<boolean>({ default: false })
const loading = ref(false)
const loaded = ref(false)
const saving = ref(false)
const testing = ref<'email' | 'bark' | null>(null)
const advancedOpen = ref(false)
const explanationOpen = ref(false)
const channels = ref<NotificationChannels | null>(null)
const recipientsText = ref('')
const form = reactive<GroupAlertPolicy>({
  groupId: '',
  enabled: false,
  concurrency: { enabled: true, threshold: 90, confirmationSeconds: 20 },
  eta: { enabled: true, threshold: 10, confirmationSeconds: 30 },
  quotaZero: { enabled: true, threshold: 0, confirmationSeconds: 0 },
  availability: { enabled: true, threshold: 0, confirmationSeconds: 0 },
  emailEnabled: false,
  emailRecipients: [],
  barkEnabled: false,
  barkLevel: null,
  barkSound: null,
  barkVolume: null,
  barkCall: null,
  updatedAt: '',
})

const smtpReady = computed(() => Boolean(channels.value?.smtp.enabled && channels.value.smtp.host && channels.value.smtp.fromEmail))
const barkReady = computed(() => channels.value?.bark.enabled && channels.value.bark.deviceKeySet)
const expired = computed(() => !!props.snapshot?.earliestResetAt && Date.parse(props.snapshot.earliestResetAt) <= (props.now ?? Date.now()))
const levelOptions = [
  { label: '继承全局', value: '' },
  { label: '普通提醒', value: 'active' },
  { label: '时效提醒', value: 'timeSensitive' },
  { label: '重要警告', value: 'critical' },
  { label: '静默记录', value: 'passive' },
]
const barkLevel = computed({ get: () => form.barkLevel ?? '', set: value => form.barkLevel = (value || null) as BarkLevel | null })
const barkCall = computed({
  get: () => form.barkCall === null ? '' : String(form.barkCall),
  set: (value: string) => form.barkCall = value === '' ? null : value === 'true',
})
const callOptions = [
  { label: '响铃：继承全局', value: '' },
  { label: '持续响铃', value: 'true' },
  { label: '普通响铃', value: 'false' },
]
const customVolume = computed({
  get: () => form.barkVolume !== null,
  set: (value: boolean) => form.barkVolume = value ? channels.value?.bark.volume ?? 5 : null,
})
const volume = computed({ get: () => form.barkVolume ?? 5, set: (value: number) => form.barkVolume = value })
const conditions = [
  { key: 'concurrency', title: '并发达到阈值', unit: '%', min: 1, max: 100 },
  { key: 'eta', title: '预计可支撑时间不足', unit: '分钟', min: 0, max: 525600 },
  { key: 'quotaZero', title: '剩余额度为 0', unit: '', min: 0, max: 0 },
  { key: 'availability', title: '可调度账号为 0', unit: '', min: 0, max: 0 },
] as const
let loadGeneration = 0

async function load() {
  if (!props.group)
    return
  const generation = ++loadGeneration
  const groupId = props.group.id
  loading.value = true
  loaded.value = false
  try {
    const [policy, notificationChannels] = await Promise.all([getGroupAlertPolicy(groupId), getNotificationChannels({ silent: true })])
    if (generation !== loadGeneration || !open.value || props.group?.id !== groupId)
      return
    Object.assign(form, policy)
    Object.assign(form.concurrency, policy.concurrency)
    Object.assign(form.eta, policy.eta)
    Object.assign(form.quotaZero, policy.quotaZero)
    Object.assign(form.availability, policy.availability)
    recipientsText.value = policy.emailRecipients.join(', ')
    channels.value = notificationChannels
    loaded.value = true
  }
  catch (error) {
    if (generation === loadGeneration)
      toast.error(errorMessage(error, '分组预警设置读取失败'))
  }
  finally {
    if (generation === loadGeneration)
      loading.value = false
  }
}

async function save(): Promise<boolean> {
  if (!props.group || !loaded.value || loading.value || saving.value)
    return false
  saving.value = true
  try {
    form.groupId = props.group.id
    form.emailRecipients = recipientsText.value.split(/[,\n]/u).map(value => value.trim()).filter(Boolean)
    const saved = await updateGroupAlertPolicy(form)
    if (open.value && props.group?.id === saved.groupId)
      Object.assign(form, saved)
    toast.success('分组预警设置已保存')
    return true
  }
  catch (error) {
    toast.error(errorMessage(error, '分组预警设置保存失败'))
    return false
  }
  finally { saving.value = false }
}

async function test(channel: 'email' | 'bark') {
  if (!props.group || testing.value || saving.value || !loaded.value)
    return
  const groupId = props.group.id
  const target = channel === 'email' ? recipientsText.value.split(/[,\n]/u)[0]?.trim() ?? '' : 'default'
  testing.value = channel
  try {
    if (!await save())
      return
    await testNotification({ channel, target, groupId })
    toast.success(channel === 'email' ? '分组测试邮件已发送' : '分组 Bark 测试已发送')
  }
  catch (error) { toast.error(errorMessage(error, '分组通知测试失败')) }
  finally { testing.value = null }
}

watch([open, () => props.group?.id], ([visible]) => {
  if (visible)
    void load()
  else
    ++loadGeneration
})
</script>

<template>
  <BaseModal v-model="open" :title="`${group?.name ?? ''} · 预警设置`" size="lg">
    <fieldset class="min-w-0 space-y-4" :disabled="loading || !loaded || saving || testing !== null">
      <div class="flex flex-wrap items-center justify-between gap-3 border-b border-cp-border-secondary pb-3">
        <div class="flex items-center gap-2 text-cp-sm">
          <BaseSwitch v-model="form.enabled" label="启用本分组预警" />启用本分组预警
        </div>
        <a href="/settings#notifications" class="text-cp-xs text-cp-primary-text">通知渠道设置</a>
      </div>
      <div class="grid gap-3 sm:grid-cols-2">
        <section
          v-for="item in conditions" :key="item.key" class="min-w-0 border-b border-cp-border-secondary pb-3"
        >
          <div class="flex items-center justify-between gap-2">
            <b class="text-cp-sm">{{ item.title }}</b><BaseSwitch v-model="form[item.key].enabled" :label="item.title" />
          </div>
          <div class="mt-2 flex flex-wrap items-center justify-between gap-2">
            <BaseNumberInput v-if="item.max" v-model="form[item.key].threshold" :label="item.title" :min="item.min" :max="item.max" :unit="item.unit" />
            <span v-else class="text-cp-xs text-cp-text-tertiary">达到条件后触发</span>
            <div class="flex flex-wrap items-center gap-1 text-cp-xs text-cp-text-tertiary">
              持续<BaseNumberInput v-model="form[item.key].confirmationSeconds" :label="`${item.title}持续确认时间`" :min="0" :max="86400" unit="秒" />
            </div>
          </div>
        </section>
      </div>
      <div class="grid gap-3 sm:grid-cols-2">
        <section class="border-b border-cp-border-secondary pb-3">
          <div class="flex items-center gap-2">
            <Mail class="size-4" /><b class="flex-1">邮件通知</b><span class="text-cp-xs" :class="smtpReady ? 'text-cp-success-text' : 'text-cp-warning-text'">{{ smtpReady ? '已配置' : '未配置' }}</span>
          </div>
          <div class="mt-2 flex items-center justify-between">
            <BaseSwitch v-model="form.emailEnabled" label="启用邮件通知" /><BaseButton size="sm" :disabled="!smtpReady || !recipientsText.trim()" :loading="testing === 'email'" @click="test('email')">
              <template #icon>
                <Send class="size-3.5" />
              </template>测试
            </BaseButton>
          </div>
          <BaseInput v-model="recipientsText" class="mt-2" placeholder="收件人，多个用逗号分隔" />
        </section>
        <section class="border-b border-cp-border-secondary pb-3">
          <div class="flex items-center gap-2">
            <BellRing class="size-4" /><b class="flex-1">Bark 铃声</b><span class="text-cp-xs" :class="barkReady ? 'text-cp-success-text' : 'text-cp-warning-text'">{{ barkReady ? '已配置' : '未配置' }}</span>
          </div>
          <div class="mt-2 flex items-center justify-between">
            <BaseSwitch v-model="form.barkEnabled" label="启用 Bark" /><BaseButton size="sm" :disabled="!barkReady" :loading="testing === 'bark'" @click="test('bark')">
              <template #icon>
                <Send class="size-3.5" />
              </template>测试
            </BaseButton>
          </div>
          <button class="mt-2 flex w-full items-center gap-2 text-left text-cp-xs text-cp-text-secondary" @click="advancedOpen = !advancedOpen">
            提醒样式覆盖<ChevronDown class="size-3.5" :class="advancedOpen ? 'rotate-180' : ''" />
          </button>
          <div v-if="advancedOpen" class="mt-2 grid gap-2">
            <BaseSelect v-model="barkLevel" :options="levelOptions" aria-label="提醒等级" />
            <BaseInput v-model="form.barkSound!" placeholder="铃声名称，留空继承全局" />
            <BaseSelect v-model="barkCall" :options="callOptions" aria-label="响铃方式" />
            <div class="flex flex-wrap items-center justify-between gap-2">
              <div class="flex items-center gap-2 text-cp-xs">
                <BaseSwitch v-model="customVolume" label="自定义音量" />自定义音量
              </div>
              <BaseNumberInput v-if="customVolume" v-model="volume" label="重要警告音量" :min="0" :max="10" />
            </div>
          </div>
        </section>
      </div>
      <details class="border-b border-cp-border-secondary pb-3" :open="explanationOpen" @toggle="explanationOpen = ($event.target as HTMLDetailsElement).open">
        <summary class="cursor-pointer text-cp-sm font-heavy">
          监控口径
        </summary>
        <div class="mt-2 space-y-2 text-cp-xs text-cp-text-secondary">
          <p v-if="stale" class="text-cp-warning-text">
            数据未更新，当前保留上次观测。
          </p>
          <p v-if="expired" class="text-cp-warning-text">
            额度窗口已到期，等待新的有效观测。
          </p>
          <p>7D 额度估算覆盖 {{ snapshot?.estimatedAccounts ?? 0 }} / {{ snapshot?.eligibleAccounts ?? 0 }} 个可调度账号；优先按自身本轮消费与已用比例计算。新号无自身估值时，参考同 Provider、套餐及窗口最新最多 3 个有效账号的平均总额度，再按自身已用比例计算剩余。不包含未来重置补充，短期限额仍可能限制使用。</p>
          <p v-if="snapshot?.remainingStatus === 'partial'">
            部分可调度账号暂无有效额度估值，当前仅汇总已可计算账号。
          </p>
          <p v-if="snapshot?.lowSample">
            样本较少，估算可能波动。
          </p>
          <p>预计过期额度依据同 Provider、同套餐最近最多 5 个未恢复失效账号的平均寿命。恢复后撤销样本；普通 Token 到期、限流和额度耗尽不计死亡。已超过平均寿命的账号跳过；全部超出或其余账号资料缺失时保持未知，不从剩余额度扣除。</p>
          <p>每分钟消耗为最近 60 秒已完成推理请求的已记录 USD，按请求授权分组范围归属，跨组可能重叠。</p>
          <p>可支撑时间按共享账号在所有分组的消耗 {{ monitorMoney(snapshot?.quotaConsumeUsdPerMinute, 'unknown', 4) }} /分计算。</p>
          <p>并发为本组 API Key 的当前占用 /（本组占用 + 可调度账号的共享空位）。其他组占用共享账号时，本组可用上限随之减少。Key 绑定多个组时占用可能重叠，不同分组的额度及并发不可直接相加。</p>
          <p>预计过期额度仅供参考，不参与预警。</p>
          <p>持续异常只通知一次，连续恢复 30 秒后才会重新允许通知。</p>
        </div>
      </details>
    </fieldset>
    <template #footer>
      <div class="flex justify-end gap-2">
        <BaseButton @click="open = false">
          取消
        </BaseButton><BaseButton variant="primary" :loading="saving" :disabled="loading || !loaded || testing !== null" @click="save">
          保存
        </BaseButton>
      </div>
    </template>
  </BaseModal>
</template>
