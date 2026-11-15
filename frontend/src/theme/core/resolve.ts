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
  PRESET_SURFACE_BACKGROUNDS,
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
    ...(resolvedColorId === 'qianxing'
      ? {
          ...themeProfiles.qianxing.customization.seed,
          colorTextBase: name === 'light' ? '#302E29' : '#EEECE5',
          colorLink: seed,
        }
      : {}),
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
  const themeMap = deriveThemeMap(theme, customization)
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
  { name, seed, seedTokens, colorId, preset }: Omit<ResolvedTheme, 'tokens'>,
  customization: ThemeCustomization,
): ThemeMap {
  // 预设底色只参与表面派生，显式设置的背景优先，编辑器与持久化的 Seed 保持不变
  const background = customization.seed?.colorBgBase ?? PRESET_SURFACE_BACKGROUNDS[colorId]?.[name] ?? seedTokens.colorBgBase
  const fills = name === 'light' ? deriveLightThemeFillMap(background, seedTokens.colorTextBase) : null
  const qianxingSurfaces = colorId === 'qianxing'
    && !customization.seed?.colorBgBase && !customization.seed?.colorTextBase
  const baseSurfaces = {
    ...deriveThemeSurfaceMap(name, background, seedTokens.colorTextBase),
    ...(qianxingSurfaces
      ? {
          colorBgLayout: name === 'light' ? '#FBFAF7' : '#262421',
          colorBgContainer: background,
          colorBgElevated: background,
          colorBorder: name === 'light' ? '#D8D1C6' : '#544C42',
          colorBorderSecondary: name === 'light' ? '#E3DED5' : '#484139',
          colorSplit: name === 'light' ? '#E3DED5' : '#484139',
        }
      : {}),
  }
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
    components: { ...deriveThemeComponentMap(
      surfaces,
      fills,
      primary,
      semantics.error,
      seedTokens.shadowStrength,
      preset,
      customization.tokenOverrides?.['--cp-input-bg'] ?? (colorId === 'qianxing' ? surfaces.colorBgContainer : undefined),
    ), ...(colorId === 'qianxing'
      ? {
          inputBg: surfaces.colorBgContainer,
          inputHoverBg: surfaces.colorBgContainer,
          inputActiveBg: surfaces.colorBgContainer,
          inputShadow: `inset 0 0 0 1px ${surfaces.colorBorder}`,
          inputHoverShadow: `inset 0 0 0 1px ${primary.colorPrimaryBorderHover}`,
          inputActiveShadow: `inset 0 0 0 1px ${primary.colorPrimary}, 0 0 0 2px ${primary.colorPrimaryContainer}`,
          buttonPrimaryColor: surfaces.colorBgContainer,
          buttonPrimaryBg: seedTokens.colorTextBase,
          buttonPrimaryHoverBg: surfaces.colorTextSecondary,
          buttonPrimaryActiveBg: seedTokens.colorTextBase,
          buttonSecondaryBg: surfaces.colorBgContainer,
          buttonSecondaryHoverBg: surfaces.colorFillQuaternary,
          buttonSecondaryActiveBg: surfaces.colorFillTertiary,
          cardShadow: 'none',
          layoutSiderShadow: 'none',
          tableRowStripeBg: surfaces.colorBgContainer,
          menuItemSelectedBg: primary.colorPrimaryContainer,
          ...(qianxingSurfaces ? { layoutSiderBg: name === 'light' ? '#F5F2EC' : '#211F1C' } : {}),
        }
      : {}) },
    dimensions: deriveThemeDimensionMap(seedTokens, {
      ...(colorId === 'qianxing' ? themeProfiles.qianxing.customization.component : {}),
      ...customization.component,
    }),
  }
}

// Keep existing callers and saved cp token keys on the same upstream compiler.
export function resolveTheme(name: ThemeName, colorId: ThemeColorId, customColor: string, customization: ThemeCustomization = DEFAULT_THEME_CUSTOMIZATION, preset: ThemePresetId = defaultThemePresetId): ResolvedTheme {
  return createTheme({ name, colorId, primaryColor: customColor, customization, preset })
}
