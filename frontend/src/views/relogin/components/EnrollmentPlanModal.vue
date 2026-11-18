<script setup lang="ts">
import type { EnrollmentEntry } from '../enrollment-plan'
import { CalendarClock, Play, Save } from '@lucide/vue'
import { computed, ref, shallowRef, useId } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseSegmented from '@/components/base/BaseSegmented.vue'
import { useAccountGroupCatalog } from '@/composables/useAccountGroupCatalog'
import { errorMessage } from '@/utils/async'
import { formatDateTime } from '@/utils/date'
import AccountSetupFields from '../../accounts/components/AccountCreateModal/AccountSetupFields.vue'
import { accountCreateEgress, accountImportSettings, accountProxyError, emptyAccountCreateForm } from '../../accounts/components/AccountCreateModal/model'
import { enrollmentSettings, previewPlanCommand } from '../enrollment-plan'

const props = defineProps<{ row: EnrollmentEntry, paused: boolean }>()
const emit = defineEmits<{ close: [], changed: [] }>()
const open = ref(true)
const plan = props.row.enrollmentPlan
const form = ref({ ...emptyAccountCreateForm(), provider: 'openai' as const, ...structuredClone(plan?.settings ?? {}) })
const mode = ref(plan?.state === 'scheduled' ? 'scheduled' : 'draft')
const modes = [{ value: 'draft', label: '仅保存', icon: Save }, { value: 'now', label: '立即执行', icon: Play }, { value: 'scheduled', label: '定时执行', icon: CalendarClock }]
const when = ref(plan?.state === 'scheduled' && plan.when ? formatDateTime(plan.when).slice(0, 16).replace(' ', 'T') : '')
const zone = Intl.DateTimeFormat().resolvedOptions().timeZone
const timeId = useId()
const fields = ref<InstanceType<typeof AccountSetupFields>>()
const { groups, loading } = useAccountGroupCatalog()
const busy = shallowRef(false)
const error = shallowRef('')
const proxyError = computed(() => accountProxyError(form.value))
const submitLabel = computed(() => mode.value === 'scheduled' ? (plan?.state === 'scheduled' ? '保存改期' : '确认定时') : mode.value === 'now' ? '立即执行' : '保存草稿')

async function submit(cancel = false) {
  if (busy.value)
    return
  busy.value = true
  error.value = ''
  try {
    const target = { id: props.row.id, revision: props.row.revision }
    if (cancel) {
      await previewPlanCommand({ ...target, mode: 'cancel' })
    }
    else {
      const time = mode.value === 'scheduled' ? new Date(when.value).getTime() : null
      if (time !== null && (!Number.isFinite(time) || time <= Date.now()))
        throw new Error('请选择晚于现在的执行时间')
      if (proxyError.value || await fields.value?.validate() === false)
        throw new Error(proxyError.value || '请检查账号设置')
      accountImportSettings(form.value)
      await previewPlanCommand({ ...target, mode: mode.value, when: time === null ? null : new Date(time).toISOString(), settings: enrollmentSettings(form.value), exit: accountCreateEgress(form.value, true) })
    }
    emit('changed')
    open.value = false
  }
  catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false }
}
</script>

<template>
  <BaseModal v-model="open" title="安排入池" :description="row.email" size="lg" :dismissible="!busy" @after-leave="emit('close')">
    <section class="mb-5 grid min-w-0 gap-3 border-b border-cp-border-secondary pb-5">
      <h3 class="m-0 text-cp-sm font-semibold">
        执行方式
      </h3>
      <BaseSegmented v-model="mode" label="执行方式" :options="modes" :disabled="busy" />
      <label v-if="mode === 'scheduled'" :for="timeId" class="grid min-w-0 gap-1 text-cp-sm sm:max-w-sm">
        <span>执行时间 <span class="text-cp-xs text-cp-text-tertiary">{{ zone }}</span></span>
        <input :id="timeId" v-model="when" aria-label="执行时间" type="datetime-local" class="min-w-0 rounded-cp border border-cp-border bg-cp-bg-container px-3 py-2 text-cp-text" :disabled="busy">
      </label>
    </section>
    <AccountSetupFields ref="fields" v-model="form" hide-provider :groups="groups" :groups-loading="loading" :disabled="busy" :proxy-error="proxyError" />
    <template #footer>
      <div class="flex w-full min-w-0 flex-wrap items-center justify-end gap-2">
        <p v-if="error || (paused && mode === 'now')" class="m-0 w-full text-cp-sm text-cp-error" role="alert">
          {{ error || '队列已暂停' }}
        </p>
        <BaseButton v-if="plan?.state === 'scheduled'" class="mr-auto" :disabled="busy" @click="submit(true)">
          取消定时
        </BaseButton>
        <BaseButton :disabled="busy" @click="open = false">
          关闭
        </BaseButton>
        <BaseButton variant="primary" :loading="busy" :disabled="paused && mode === 'now'" @click="submit()">
          {{ submitLabel }}
        </BaseButton>
      </div>
    </template>
  </BaseModal>
</template>
