<script setup lang="ts">
import type { ExcelImageSettings } from '../composables/useExcelImageSettings'
import type { RequestTuning } from '@/api/modules/settings'
import { computed } from 'vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { DEFAULT_EXCEL_MODELS_INPUT } from '@/utils/excel-defaults'

const props = defineProps<{ disabled?: boolean, images: ExcelImageSettings }>()
const models = defineModel<string>('models', { required: true })
const requestTuning = defineModel<RequestTuning>('requestTuning', { required: true })
const { mode: imageMode, relayUrl: imageRelayUrl, fields } = props.images
const warningFields = fields.filter(field => field.section === 'warn')
const limitFields = fields.filter(field => field.section === 'limits')
const relayFields = fields.filter(field => field.section === 'relay')
const imageModes = [
  { value: 'inherit', label: '继承启动配置', description: '未配置中转网址时使用 BPS 原生附件上传' },
  { value: 'native', label: 'BPS 原生附件上传', description: '不使用公网图片中转' },
  { value: 'relay', label: '临时 HTTPS 中转', description: '使用本实例的公网域名提供临时图片' },
]
const imagePolicies = [
  { value: 'off', label: '保持现状' },
  { value: 'auto_compact', label: '自动压缩历史' },
  { value: 'warn', label: '预警拦截并预留压缩空间' },
]
const totalSizeHint = computed(() => requestTuning.value.excelImageLimitPolicy === 'off'
  ? '内联图片按去重后的解码字节计量；仍受通用请求体上限约束。'
  : '启用图片限额策略后，原始历史中的重复内联图片也计入解码字节预算；仍受通用请求体上限约束。')
</script>

<template>
  <BaseCard title="Excel 配置" description="仅作用于选择 Excel 上游的请求；重试、账号切换和调度继续遵循通用配置。">
    <section aria-labelledby="excel-default-models-title">
      <h3 id="excel-default-models-title" class="mb-3 mt-0 text-sm font-medium text-cp-text-secondary">
        默认模型
      </h3>
      <BaseForm class="max-w-6xl sm:grid-cols-2">
        <BaseFormItem label="Excel 默认模型列表" description="英文逗号分隔。仅用于选择继承全局模型的账号，不会自动开启账号 Excel 模式。">
          <BaseInput v-model="models" aria-label="Excel 默认模型列表" :placeholder="DEFAULT_EXCEL_MODELS_INPUT" :disabled="disabled" />
        </BaseFormItem>
      </BaseForm>
    </section>

    <section class="mt-5 border-t border-cp-border pt-4" aria-labelledby="excel-image-transport-title">
      <h3 id="excel-image-transport-title" class="mb-3 mt-0 text-sm font-medium text-cp-text-secondary">
        图片传输
      </h3>
      <BaseForm class="max-w-6xl sm:grid-cols-2">
        <BaseFormItem label="图片传输方式" description="按所选模式执行，不自动切换或回退。继承模式由服务启动配置决定。">
          <BaseSelect v-model="imageMode" class="w-full min-w-0" :options="imageModes" :disabled="disabled" aria-label="Excel 图片传输方式" />
        </BaseFormItem>
        <BaseFormItem v-if="imageMode === 'relay'" label="公网 HTTPS 访问地址" description="填写指向本实例的公网域名，不附加 /v1 等路径；反向代理需转发 /_cpr/excel-images/。">
          <BaseInput v-model="imageRelayUrl" :disabled="disabled" type="url" placeholder="https://images.example.com" aria-label="Excel 公网 HTTPS 访问地址" />
        </BaseFormItem>
      </BaseForm>
    </section>

    <section class="mt-5 border-t border-cp-border pt-4" aria-labelledby="excel-image-policy-title">
      <h3 id="excel-image-policy-title" class="mb-3 mt-0 text-sm font-medium text-cp-text-secondary">
        图片限额处理策略
      </h3>
      <BaseForm class="max-w-6xl sm:grid-cols-2">
        <BaseFormItem label="图片限额处理策略" description="自动压缩的是旧会话历史，不是图片分辨率；需要稳定会话及受支持的 Codex 客户端，会产生额外调用与 Token 用量。">
          <BaseSelect v-model="requestTuning.excelImageLimitPolicy" class="w-full min-w-0" :options="imagePolicies" :disabled="disabled" aria-label="Excel 图片限额处理策略" />
        </BaseFormItem>
        <template v-if="requestTuning.excelImageLimitPolicy === 'warn'">
          <BaseFormItem v-for="field in warningFields" :key="field.key" :label="field.label" :error="field.error.value">
            <BaseInput v-model="field.input.value" :aria-label="`${field.label}（${field.unit}）`" :disabled="disabled" type="number" :min="field.min" :max="field.max" step="1">
              <template #suffix>
                {{ field.unit }}
              </template>
            </BaseInput>
          </BaseFormItem>
          <p class="m-0 text-cp-xs text-cp-text-tertiary sm:col-span-2">
            预留量 &lt; 预警余量 &lt; 图片数上限。例如上限 20 张、预警 8 张、预留 3 张：12 张时首次预警，重试可继续至 17 张。
          </p>
        </template>
      </BaseForm>
    </section>

    <section class="mt-5 border-t border-cp-border pt-4" aria-labelledby="excel-image-limits-title">
      <h3 id="excel-image-limits-title" class="mb-3 mt-0 text-sm font-medium text-cp-text-secondary">
        图片大小与数量
      </h3>
      <BaseForm class="max-w-6xl sm:grid-cols-2">
        <BaseFormItem
          v-for="field in limitFields" :key="field.key" :label="field.label" :error="field.error.value"
          :description="field.key === 'excelImageTotalBytes' ? totalSizeHint : field.key === 'excelImageMaxCount' ? '统计有效请求中的图片引用，包含历史消息和工具图片。' : undefined"
        >
          <BaseInput v-model="field.input.value" :aria-label="`${field.label}（${field.unit}）`" :disabled="disabled" type="number" :min="field.min / field.scale" :max="field.max / field.scale" :step="field.scale === 1 ? '1' : 'any'">
            <template #suffix>
              {{ field.unit }}
            </template>
          </BaseInput>
        </BaseFormItem>
      </BaseForm>
      <p class="mb-0 mt-3 text-cp-xs text-cp-text-tertiary">
        1 MiB = 1,048,576 字节。上述限制也适用于原生附件模式，不改变通用请求体上限。
      </p>
    </section>

    <section v-show="imageMode !== 'native'" class="mt-5 border-t border-cp-border pt-4" aria-labelledby="excel-image-relay-title">
      <h3 id="excel-image-relay-title" class="mb-3 mt-0 text-sm font-medium text-cp-text-secondary">
        HTTPS 中转资源
      </h3>
      <p class="mb-3 mt-0 text-cp-xs text-cp-text-tertiary">
        仅实际使用 HTTPS 中转时生效。暂存容量属于本实例的临时图片总容量；链接有效期不是模型记忆图片的时长。
      </p>
      <BaseForm class="max-w-6xl sm:grid-cols-2">
        <BaseFormItem v-for="field in relayFields" :key="field.key" :label="field.label" :error="field.error.value">
          <BaseInput v-model="field.input.value" :aria-label="`${field.label}（${field.unit}）`" :disabled="disabled" type="number" :min="field.min / field.scale" :max="field.max / field.scale" :step="field.scale === 1 ? '1' : 'any'">
            <template #suffix>
              {{ field.unit }}
            </template>
          </BaseInput>
        </BaseFormItem>
      </BaseForm>
    </section>
  </BaseCard>
</template>
