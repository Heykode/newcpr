import DOMPurify from 'dompurify'
import { Marked } from 'marked'
import markedAlert from 'marked-alert'

const marked = new Marked({
  async: false,
  breaks: false,
  gfm: true,
}).use(markedAlert())

export function renderMarkdown(source?: string | null) {
  const html = marked
    .parse(source ?? '', { async: false })
    .replaceAll('<a ', '<a target="_blank" rel="noreferrer" ')

  const content = DOMPurify.sanitize(html, {
    ADD_ATTR: ['target', 'rel'],
    FORBID_TAGS: ['slot'],
    RETURN_DOM_FRAGMENT: true,
  })
  const blocks: { slot: string, text: string }[] = []
  // 只在清洗后的代码块旁创建插槽，让复制按钮沿用组件库主题，不受正文样式影响。
  for (const pre of content.querySelectorAll('pre')) {
    const code = pre.querySelector(':scope > code:only-child')
    if (!code)
      continue
    const wrapper = document.createElement('div')
    wrapper.className = 'markdown-code-block'
    const slot = document.createElement('slot')
    slot.name = `code-copy-${blocks.length}`
    pre.replaceWith(wrapper)
    wrapper.append(pre, slot)
    blocks.push({ slot: slot.name, text: code.textContent.replace(/\n$/u, '') })
  }
  return { content, blocks }
}
