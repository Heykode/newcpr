import type {
  ResolvedTheme,
  ResolvedThemeSeedTokens,
  ThemeAppearanceSeed,
  ThemeColorId,
  ThemeColorPreset,
  ThemeColorPresetId,
  ThemeCustomization,
  ThemeMap,
  ThemeMode,
  ThemeName,
  ThemeOptions,
  ThemePresetId,
  ThemeSeedOverrides,
  ThemeTokens,
} from '../types'
import { normalizeHexColor } from '../../utils/color'
import { deriveCustomThemeAppearance } from '../derive/appearance'
import {
  deriveLightThemeFillMap,
  deriveThemeAliasMap,
  deriveThemeDataMap,
  deriveThemeLinkMap,
  deriveThemePresetColorMap,
  deriveThemePrimaryMap,
  deriveThemeSemanticMap,
  deriveThemeSurfaceMap,
  ensureThemeSurfaceContrast,
} from '../derive/colors'
import {
  deriveThemeComponentMap,
  deriveThemeDimensionMap,
  deriveThemeShadowMap,
} from '../derive/components'
import {
  DARK_CONTAINER_BASE,
  DARK_TEXT_BASE,
  DEFAULT_CUSTOM_THEME_COLOR,
  DEFAULT_DIMENSIONS,
  DEFAULT_SEED_COLORS,
  DEFAULT_THEME_CUSTOMIZATION,
  defaultThemePresetId,
  LIGHT_CONTAINER_BASE,
  LIGHT_TEXT_BASE,
  THEME_COLOR_PRESETS,
  themePresets,
} from './constants'
import { normalizeThemeCustomization } from './normalize'
import { createThemeTokens } from './tokens'

export function resolveThemeName(mode: ThemeMode, prefersDark: boolean): ThemeName {
  if (mode === 'system')
    return prefersDark ? 'dark' : 'light'
  return mode
}

export function resolveThemeSeed(colorId: ThemeColorId, customColor: string): string {
  if (colorId === 'custom')
    return normalizeHexColor(customColor) ?? DEFAULT_CUSTOM_THEME_COLOR
  return themeColorPreset(colorId).seed
}

function resolveThemeInputs({
  name = 'light',
  preset = defaultThemePresetId,
  colorId,
  primaryColor,
  customization = DEFAULT_THEME_CUSTOMIZATION,
}: ThemeOptions) {
  const selectedPreset = themePresets.find(item => item.id === preset) ?? themePresets[0]!
  const resolvedColorId = colorId ?? selectedPreset.colorId
  const seed = resolveThemeSeed(resolvedColorId, primaryColor ?? selectedPreset.primaryColor)
  const defaults = selectedPreset.customization?.[name]
  const overrides = normalizeThemeCustomization(customization)
  // 预设提供基线，用户覆盖始终最后应用；Store 与页面只保存输入配置。
  const normalizedCustomization: ThemeCustomization = {
    seed: { ...defaults?.seed, ...overrides.seed },
    component: { ...defaults?.component, ...overrides.component },
    tokenOverrides: { ...defaults?.tokenOverrides, ...overrides.tokenOverrides },
  }
  const appearance = resolvedColorId === 'custom'
    ? deriveCustomThemeAppearance(name, seed)
    : themeColorPreset(resolvedColorId).appearance?.[name]
  const seedTokens = resolveThemeSeedTokens(
    name,
    seed,
    normalizedCustomization.seed,
    appearance,
  )
  return { name, seed, seedTokens, preset: selectedPreset.id, colorId: resolvedColorId, customization: normalizedCustomization }
}

export function createThemeSeedTokens(options: ThemeOptions = {}): ResolvedThemeSeedTokens {
  return resolveThemeInputs(options).seedTokens
}

export function createTheme(options: ThemeOptions = {}): ResolvedTheme {
  const { customization, ...theme } = resolveThemeInputs(options)
  const themeMap = deriveThemeMap(theme.name, theme.seed, theme.seedTokens, customization, theme.preset)
  const tokensWithOverrides: ThemeTokens = {
    ...createThemeTokens(themeMap),
    ...customization.tokenOverrides,
  }

  return {
    ...theme,
    tokens: tokensWithOverrides,
  }
}

export function themeColorPreset(id: ThemeColorPresetId): ThemeColorPreset {
  return THEME_COLOR_PRESETS.find(preset => preset.id === id) ?? THEME_COLOR_PRESETS[0]!
}

function deriveThemeMap(
  name: ThemeName,
  seed: string,
  seedTokens: ResolvedThemeSeedTokens,
  customization: ThemeCustomization,
  preset: ThemePresetId,
): ThemeMap {
  const fills = name === 'light' ? deriveLightThemeFillMap(seedTokens) : null
  const baseSurfaces = deriveThemeSurfaceMap(name, seedTokens)
  const primary = deriveThemePrimaryMap(seed, name, baseSurfaces.colorBgContainer)
  const surfaces = ensureThemeSurfaceContrast(baseSurfaces, fills, primary)
  const aliases = deriveThemeAliasMap(surfaces)
  const link = deriveThemeLinkMap(seedTokens.colorLink, name, surfaces.colorBgContainer)
  const semantics = deriveThemeSemanticMap(name, surfaces, seedTokens, fills, primary)

  return {
    surfaces,
    aliases,
    primary,
    link,
    semantics,
    presetColors: deriveThemePresetColorMap(name, surfaces.colorBgContainer, seedTokens),
    data: deriveThemeDataMap(aliases, semantics),
    shadows: deriveThemeShadowMap(name, seedTokens.shadowStrength),
    components: deriveThemeComponentMap(
      surfaces,
      fills,
      primary,
      semantics.error,
      seedTokens.shadowStrength,
      preset,
    ),
    dimensions: deriveThemeDimensionMap(seedTokens, customization.component, preset),
  }
}

export function resolveThemeSeedTokens(
  theme: ThemeName,
  colorPrimary: string,
  overrides?: ThemeSeedOverrides,
  appearance?: ThemeAppearanceSeed,
): ResolvedThemeSeedTokens {
  return {
    colorPrimary,
    colorSuccess: overrides?.colorSuccess ?? DEFAULT_SEED_COLORS.colorSuccess,
    colorWarning: overrides?.colorWarning ?? DEFAULT_SEED_COLORS.colorWarning,
    colorError: overrides?.colorError ?? DEFAULT_SEED_COLORS.colorError,
    colorInfo: overrides?.colorInfo ?? DEFAULT_SEED_COLORS.colorInfo,
    colorLink: overrides?.colorLink ?? colorPrimary,
    colorTextBase: overrides?.colorTextBase
      ?? appearance?.colorTextBase
      ?? (theme === 'dark' ? DARK_TEXT_BASE : LIGHT_TEXT_BASE),
    colorBgBase: overrides?.colorBgBase
      ?? appearance?.colorBgBase
      ?? (theme === 'dark' ? DARK_CONTAINER_BASE : LIGHT_CONTAINER_BASE),
    fontSize: overrides?.fontSize ?? DEFAULT_DIMENSIONS.fontSize,
    sizeUnit: overrides?.sizeUnit ?? DEFAULT_DIMENSIONS.sizeUnit,
    sizeStep: overrides?.sizeStep ?? DEFAULT_DIMENSIONS.sizeStep,
    controlHeight: overrides?.controlHeight ?? DEFAULT_DIMENSIONS.controlHeight,
    borderRadius: overrides?.borderRadius ?? DEFAULT_DIMENSIONS.borderRadius,
    shadowStrength: overrides?.shadowStrength ?? DEFAULT_DIMENSIONS.shadowStrength,
  }
}

// Keep existing callers and saved cp token keys on the same upstream compiler.
export function resolveTheme(name: ThemeName, colorId: ThemeColorId, customColor: string, customization: ThemeCustomization = DEFAULT_THEME_CUSTOMIZATION, preset: ThemePresetId = defaultThemePresetId): ResolvedTheme {
  return createTheme({ name, colorId, primaryColor: customColor, customization, preset })
}
