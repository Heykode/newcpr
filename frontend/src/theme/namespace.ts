// Native CPR uses cp in its existing CSS and saved theme overrides.
export const themePrefix = 'cp'

export function getThemeVarName(name: string): `--${string}` {
  return `--${themePrefix}-${name}`
}

export function createThemeVars(tokens: Readonly<Record<string, string>>): Record<`--${string}`, string> {
  return Object.fromEntries(Object.entries(tokens).map(([name, value]) => [getThemeVarName(name), value]))
}
