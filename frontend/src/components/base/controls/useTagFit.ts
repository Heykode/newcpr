import type { Ref } from 'vue'
import { useResizeObserver } from '@vueuse/core'
import { shallowRef, watch } from 'vue'

export function useTagFit(
  list: Readonly<Ref<HTMLElement | null>>,
  samples: Readonly<Ref<HTMLElement | null>>,
  labels: Readonly<Ref<string[]>>,
  total: () => number,
) {
  const count = shallowRef(0)

  function measure() {
    if (!samples.value || !list.value)
      return
    const [tags, counts, input] = samples.value.children
    if (!tags || !counts || !input)
      return
    const gap = Number.parseFloat(getComputedStyle(list.value).columnGap) || 0
    const widthOf = (el: Element) => Number.parseFloat(getComputedStyle(el).width)
    const available = widthOf(list.value) - widthOf(input)
    const badges = Array.from(counts.children, widthOf)
    let width = 0
    let fit = 0
    // 逐个累加前缀；最后一项放下时不再需要 +N，不能因前一项加计数放不下就提前结束。
    for (let index = 0; index < tags.children.length; index++) {
      width += widthOf(tags.children[index]!) + gap
      if (width > available)
        break
      const left = total() - index - 1
      const summary = left ? (badges[String(left).length - 1] ?? 0) + gap : 0
      if (width + summary <= available)
        fit = index + 1
    }
    count.value = fit
  }

  // 同时观察容器与样本，覆盖父级改宽、字体加载和标签内容更新。
  useResizeObserver(() => samples.value ? [list.value, samples.value] : [], measure)
  watch([samples, labels, total], measure, { flush: 'post' })
  return count
}
