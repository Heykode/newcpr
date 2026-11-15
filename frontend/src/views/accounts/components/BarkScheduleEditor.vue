<script setup lang="ts">
import type { BarkSchedule } from '@/api'
import { ArrowDown, ArrowUp, Plus, Trash2 } from '@lucide/vue'
import BaseButton from '@/components/base/BaseButton.vue'
import FormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseNumberInput from '@/components/base/BaseNumberInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import { barkLevelHints, barkLevelOptions } from '@/utils/notification-presentation'

const props = defineProps<{ defaultVolume: number }>()
const schedule = defineModel<BarkSchedule>({ required: true })
const levelOptions = [{ value: '', label: '继承默认提醒' }, ...barkLevelOptions]
const callOptions = [{ value: '', label: '继承默认提醒' }, { value: 'false', label: '普通响铃' }, { value: 'true', label: '持续响铃约 30 秒' }]

function addRule() {
  if (schedule.value.rules.length >= 24)
    return
  schedule.value.rules.push({
    id: Array.from(crypto.getRandomValues(new Uint8Array(16)), byte => byte.toString(16).padStart(2, '0')).join(''),
    name: `提醒时段 ${schedule.value.rules.length + 1}`,
    enabled: true,
    startTime: '09:00',
    endTime: '18:00',
    level: null,
    sound: null,
    volume: null,
    call: null,
  })
}

function moveRule(index: number, direction: number) {
  const target = index + direction
  if (target < 0 || target >= schedule.value.rules.length)
    return
  const [rule] = schedule.value.rules.splice(index, 1)
  if (rule)
    schedule.value.rules.splice(target, 0, rule)
}
</script>

<template>
  <section class="space-y-3" aria-label="Bark 提醒时间">
    <div class="flex flex-wrap items-center justify-between gap-2">
      <b class="text-cp-sm">Bark 提醒时间</b>
      <BaseButton size="sm" :disabled="schedule.rules.length >= 24" @click="addRule">
        <Plus class="size-3.5" />添加提醒时段
      </BaseButton>
    </div>
    <div class="grid items-center gap-3 sm:grid-cols-2">
      <div class="flex items-center gap-2 text-cp-sm">
        <BaseSwitch v-model="schedule.defaultEnabled" label="启用默认时段提醒" />时段外使用默认提醒
      </div>
      <FormItem label="提醒时区" description="例如 Asia/Shanghai">
        <BaseInput v-model="schedule.timezone" placeholder="Asia/Shanghai" />
      </FormItem>
    </div>
    <p class="text-cp-xs text-cp-text-tertiary">
      时间段从上到下匹配，优先使用第一条启用且命中的规则。支持跨午夜；关闭默认提醒后，时段外不推送。预警阈值沿用本组设置。
    </p>
    <section v-for="(rule, index) in schedule.rules" :key="rule.id" class="min-w-0 space-y-3 rounded-cp-lg border border-cp-border-secondary p-3" :aria-label="`Bark 时段 ${index + 1}`">
      <div class="flex items-center gap-2">
        <BaseSwitch v-model="rule.enabled" :label="`启用时段 ${index + 1}`" />
        <b class="min-w-0 flex-1 truncate text-cp-sm">{{ rule.name || `时段 ${index + 1}` }}</b>
        <BaseIconButton :label="`上移时段 ${index + 1}`" :disabled="index === 0" size="sm" @click="moveRule(index, -1)">
          <ArrowUp class="size-3.5" />
        </BaseIconButton>
        <BaseIconButton :label="`下移时段 ${index + 1}`" :disabled="index === schedule.rules.length - 1" size="sm" @click="moveRule(index, 1)">
          <ArrowDown class="size-3.5" />
        </BaseIconButton>
        <BaseIconButton :label="`删除时段 ${index + 1}`" size="sm" @click="schedule.rules.splice(index, 1)">
          <Trash2 class="size-3.5" />
        </BaseIconButton>
      </div>
      <div class="grid min-w-0 gap-3 sm:grid-cols-2">
        <FormItem label="时段名称" class="sm:col-span-2">
          <BaseInput v-model="rule.name" maxlength="80" />
        </FormItem>
        <FormItem label="开始时间">
          <BaseInput v-model="rule.startTime" type="time" />
        </FormItem>
        <FormItem label="结束时间">
          <BaseInput v-model="rule.endTime" type="time" />
        </FormItem>
        <FormItem label="时段提醒等级" :description="rule.level ? barkLevelHints[rule.level] : '使用本组默认等级；未覆盖时继承全局。'">
          <BaseSelect :model-value="rule.level ?? ''" :options="levelOptions" @update:model-value="rule.level = barkLevelOptions.find(option => option.value === $event)?.value ?? null" />
        </FormItem>
        <FormItem label="时段响铃时长" description="持续响铃只延长时长，不会改变静音设置。">
          <BaseSelect :model-value="rule.call === null ? '' : String(rule.call)" :options="callOptions" @update:model-value="rule.call = $event === '' ? null : $event === 'true'" />
        </FormItem>
        <FormItem label="时段铃声">
          <BaseInput :model-value="rule.sound ?? ''" placeholder="留空继承默认铃声" maxlength="128" @update:model-value="rule.sound = $event || null" />
        </FormItem>
        <div class="space-y-2">
          <div class="flex items-center gap-2 text-cp-xs">
            <BaseSwitch :model-value="rule.volume !== null" label="自定义时段重要警告音量" @update:model-value="rule.volume = $event ? props.defaultVolume : null" />自定义重要警告音量
          </div>
          <BaseNumberInput v-if="rule.volume !== null" v-model="rule.volume" label="时段重要警告音量" :min="0" :max="10" />
          <span v-else class="text-cp-xs text-cp-text-tertiary">继承默认 · {{ defaultVolume }}</span>
        </div>
      </div>
    </section>
    <p class="text-cp-xs text-cp-text-tertiary">
      同一轮异常只在提醒变强时补发。测试按当前时间匹配规则，免打扰时段不发送。
    </p>
  </section>
</template>
