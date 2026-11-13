// 与官方表单规则一致；后端解析器负责 Cron 字段的取值与日历校验。
export function validWarmupExpressions(expressions: string[], enabled: boolean) {
  return (!enabled || expressions.length > 0)
    && expressions.every(expression => /^[^\s;]+(?: +[^\s;]+){4}$/u.test(expression))
}

export function warmupScheduleBytes(expressions: string[]) {
  return new TextEncoder().encode(expressions.join(';')).length
}
