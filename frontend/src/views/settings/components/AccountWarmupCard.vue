<script setup lang="ts">
import { Clock, Sparkles } from '@lucide/vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseInputTag from '@/components/base/BaseInputTag.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import { t } from '@/i18n'

const enabled = defineModel<boolean>('enabled', { required: true })
const cron = defineModel<string[]>('cron', { required: true })
const model = defineModel<string>('model', { required: true })
</script>

<template>
  <BaseCard
    title="账号预激活"
    description="在指定时间向空闲的 Codex OAuth 账号发送一次请求，尝试启动额度窗口"
  >
    <BaseForm class="max-w-6xl sm:grid-cols-2">
      <BaseSwitch
        v-model="enabled"
        class="col-span-full justify-self-start"
        label="启用预激活"
        show-label
      />

      <BaseFormItem
        prop="accountWarmupCron"
        label="Cron 表达式（部署时区）"
        :required="enabled"
        description="回车添加规则，支持多个；按部署时区执行，同一分钟只预热一次"
      >
        <BaseInputTag
          v-model="cron"
          :disabled="!enabled"
          aria-label="预激活 Cron 表达式"
          placeholder="0 8 * * *"
          collapse-tags
          collapse-tags-tooltip
          spellcheck="false"
        >
          <template #prefix>
            <Clock class="size-4" />
          </template>
        </BaseInputTag>
      </BaseFormItem>

      <BaseFormItem
        prop="accountWarmupModel"
        :label="t('settings.warmup.modelLabel')"
        :description="t('settings.warmup.modelHelp')"
        :required="enabled"
      >
        <BaseInput
          v-model="model"
          :disabled="!enabled"
          :aria-label="t('settings.warmup.modelLabel')"
          :placeholder="t('settings.warmup.modelPlaceholder')"
        >
          <template #prefix>
            <Sparkles class="size-4" />
          </template>
        </BaseInput>
      </BaseFormItem>
    </BaseForm>
  </BaseCard>
</template>
