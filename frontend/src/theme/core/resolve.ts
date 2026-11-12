import type {
  ResolvedTheme,
  ResolvedThemeSeedTokens,
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
import { pick } from 'es-toolkit'
import { normalizeHexColor } from '../../utils/color'
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
  DEFAULT_CUSTOM_THEME_COLOR,
  DEFAULT_SEED_DIMENSIONS,
  DEFAULT_THEME_CUSTOMIZATION,
  defaultThemePresetId,
  SEED_COLORS_BY_SCHEME,
  THEME_COLOR_PRESETS,
  THEME_SEED_COLOR_KEYS,
  themePresets,
  themeProfiles,
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
  const normalizedCustomization = normalizeThemeCustomization(customization)
  const seedTokens: ResolvedThemeSeedTokens = {
    colorPrimary: seed,
    ...SEED_COLORS_BY_SCHEME[name],
    ...DEFAULT_SEED_DIMENSIONS,
    ...normalizedCustomization.seed,
  }
  return { name, seed, seedTokens, preset: selectedPreset.id, colorId: resolvedColorId, customization: normalizedCustomization }
}

/** 保留预设未指定的颜色；未编辑的默认值继续跟随明暗模式 */
export function resolveThemeProfile(colorId: ThemeColorPresetId, currentSeed: ThemeSeedOverrides = {}) {
  const { preset, customization } = themeProfiles[colorId]
  return {
    preset,
    customization: normalizeThemeCustomization({
      ...customization,
      seed: {
        ...pick(currentSeed, THEME_SEED_COLOR_KEYS),
        ...customization.seed,
      },
    }),
  }
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
    dimensions: deriveThemeDimensionMap(seedTokens, customization.component),
  }
}

// Keep existing callers and saved cp token keys on the same upstream compiler.
export function resolveTheme(name: ThemeName, colorId: ThemeColorId, customColor: string, customization: ThemeCustomization = DEFAULT_THEME_CUSTOMIZATION, preset: ThemePresetId = defaultThemePresetId): ResolvedTheme {
  return createTheme({ name, colorId, primaryColor: customColor, customization, preset })
}
