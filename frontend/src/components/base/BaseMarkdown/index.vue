<script setup lang="ts">
import { onMounted, shallowRef, useTemplateRef, watch } from 'vue'
import { getThemeVarName } from '@/theme/namespace'
import MarkdownCodeCopy from './components/MarkdownCodeCopy.vue'
import markdownStyles from './styles/markdown.css?inline'
import { renderMarkdown } from './utils/markdown'

const props = defineProps<{
  source?: string | null
}>()

const host = useTemplateRef<HTMLElement>('host')
const blocks = shallowRef<ReturnType<typeof renderMarkdown>['blocks']>([])

onMounted(() => {
  if (!host.value)
    return

  const shadowRoot = host.value.attachShadow({ mode: 'open' })
  const style = document.createElement('style')
  const content = document.createElement('div')
  content.className = 'markdown-body'
  // 内联样式打包为 JS 字符串后不会再经过消费方的 CSS 转换，按当前命名空间绑定变量。
  style.textContent = markdownStyles.replace(/--z-([a-z0-9-]+)/gu, (_, token: string) => getThemeVarName(token))
  shadowRoot.append(style, content)
  watch(() => props.source, (source) => {
    const rendered = renderMarkdown(source)
    content.replaceChildren(rendered.content)
    blocks.value = rendered.blocks
  }, { immediate: true })
})
</script>

<template>
  <div ref="host" class="min-w-0">
    <MarkdownCodeCopy v-for="block in blocks" :key="block.slot + block.text" v-bind="block" />
  </div>
</template>
