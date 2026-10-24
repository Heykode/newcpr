<script setup lang="ts">
import { CalendarClock, Save } from '@lucide/vue'
import BaseButton from '@/components/base/BaseButton.vue'

import BaseCard from '@/components/base/BaseCard.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { t } from '@/i18n'

interface ScheduleForm {
  scheduleEnabled: boolean
  cronExpression: string
  scheduleTimezone: string
  retentionDays: string
  retentionCount: string
}

defineProps<{
  loading: boolean
  saving: boolean
  storageReady: boolean
}>()

const emit = defineEmits<{
  save: []
}>()

const schedule = defineModel<ScheduleForm>('schedule', { required: true })

const TIMEZONE_OPTIONS = [
  { label: 'Asia/Shanghai（中国标准时间）', value: 'Asia/Shanghai' },
  { label: 'UTC', value: 'UTC' },
  { label: 'America/New_York（东部时间）', value: 'America/New_York' },
  { label: 'Europe/London', value: 'Europe/London' },
  { label: 'Asia/Tokyo', value: 'Asia/Tokyo' },
]
</script>

<template>
  <BaseCard
    title="定时备份"
    description="配置自动定时备份"
  >
    <template #actions>
      <BaseButton variant="primary" :loading="saving" :disabled="loading" @click="emit('save')">
        <template #icon>
          <Save class="size-4" />
        </template>
        {{ saving ? '保存中...' : '保存' }}
      </BaseButton>
    </template>

    <div class="@container">
      <BaseForm class="max-w-6xl @min-[640px]:grid-cols-2">
        <div class="col-span-2 flex items-center gap-4 @max-[640px]:col-span-1">
          <BaseCheckbox
            v-model="schedule.scheduleEnabled"
            :disabled="!storageReady"
            :label="t('settings.backup.schedule.enableScheduledBackup')"
            show-label
          />
        </div>

        <BaseFormItem label="时区" description="显式 IANA 时区">
          <BaseSelect v-model="schedule.scheduleTimezone" :options="TIMEZONE_OPTIONS" />
        </BaseFormItem>

        <BaseFormItem
          :label="t('settings.backup.schedule.cronExpression')"
          :description="t('settings.backup.schedule.cronHelp')"
        >
          <BaseInput v-model="schedule.cronExpression" :aria-label="t('settings.backup.schedule.cronExpression')">
            <template #prefix>
              <CalendarClock class="size-4" />
            </template>
          </BaseInput>
        </BaseFormItem>

        <BaseFormItem label="备份过期天数" description="超过此天数自动删除，0 = 永不过期">
          <BaseInput v-model="schedule.retentionDays" aria-label="备份过期天数" type="number" min="0" />
        </BaseFormItem>

        <BaseFormItem
          label="最大保留份数"
          description="最多保留的备份数量，0 = 不限制"
        >
          <BaseInput v-model="schedule.retentionCount" aria-label="最大保留份数" type="number" min="0" />
        </BaseFormItem>
      </BaseForm>
    </div>
  </BaseCard>
</template>
