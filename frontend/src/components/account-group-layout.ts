const GAP = 6

function fitsTwoRows(widths: number[], available: number) {
  let rows = 1
  let used = 0
  for (const width of widths) {
    if (used > 0 && used + GAP + width > available) {
      rows += 1
      used = 0
    }
    if (rows > 2)
      return false
    used += (used > 0 ? GAP : 0) + width
  }
  return true
}

export function accountGroupLayout(widths: number[], available: number, overflowWidth: number) {
  if (available <= 0)
    return { count: Math.min(widths.length, 2), maxWidth: undefined }
  if (fitsTwoRows(widths.map(width => Math.min(width, available)), available))
    return { count: widths.length, maxWidth: available }

  // Reserve room for the overflow count beside even a long final label.
  const maxWidth = Math.max(1, available - overflowWidth - GAP)
  const fitted: number[] = []
  let count = 0
  for (const width of widths) {
    fitted.push(Math.min(width, maxWidth))
    if (!fitsTwoRows([...fitted, overflowWidth], available))
      break
    count += 1
  }
  return { count, maxWidth }
}
