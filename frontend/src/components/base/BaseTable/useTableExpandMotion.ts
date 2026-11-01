import { usePreferredReducedMotion } from '@vueuse/core'
import { onScopeDispose } from 'vue'

export function useTableExpandMotion() {
  const preferredMotion = usePreferredReducedMotion()
  const animations = new Map<HTMLElement, Animation>()

  function transition(row: Element, done: () => void, expanding: boolean) {
    const content = row.querySelector<HTMLElement>('[data-expanded-content]')!
    const height = content.getBoundingClientRect().height
    const opacity = getComputedStyle(content).opacity
    animations.get(content)?.cancel()
    if (preferredMotion.value === 'reduce') {
      done()
      return
    }

    // 动画结束后恢复内容自然高度，异步内容与窗口缩放仍由正常布局处理
    const animation = content.animate({
      height: expanding ? ['0px', `${content.scrollHeight}px`] : [`${height}px`, '0px'],
      opacity: expanding ? ['0', '1'] : [opacity, '0'],
      overflow: ['hidden', 'hidden'],
    }, {
      duration: 200,
      easing: 'cubic-bezier(0.2, 0, 0, 1)',
    })
    animations.set(content, animation)
    const clear = () => {
      if (animations.get(content) === animation)
        animations.delete(content)
    }
    animation.onfinish = () => {
      clear()
      done()
    }
    animation.oncancel = clear
  }

  onScopeDispose(() => {
    animations.forEach(animation => animation.cancel())
    animations.clear()
  })

  return {
    enter: (row: Element, done: () => void) => transition(row, done, true),
    leave: (row: Element, done: () => void) => transition(row, done, false),
  }
}
