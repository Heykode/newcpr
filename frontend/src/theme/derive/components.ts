import type {
  FunctionalColorMap,
  ResolvedThemeSeedTokens,
  ThemeComponentMap,
  ThemeComponentOverrides,
  ThemeDimensionMap,
  ThemeFillMap,
  ThemeName,
  ThemePresetId,
  ThemePrimaryMap,
  ThemeShadowMap,
  ThemeSurfaceMap,
} from '../types'
import {
  compositeOnBackground,
  ensureContrast,
  mix,
  scaleShadowAlpha,
  withAlpha,
} from '../core/color'
import {
  BLACK,
  DEFAULT_DIMENSIONS,
  LIGHT_SHADOW_BASE,
  WHITE,
} from '../core/constants'

type SurfaceComponentMap = Omit<ThemeComponentMap, `buttonPrimary${string}` | `switchUnchecked${string}` | 'tableRowSelectedHoverBg'>

const SHADOW_MAP_DERIVERS = {
  light: deriveLightShadowMap,
  dark: deriveDarkShadowMap,
} satisfies Record<ThemeName, (shadowStrength: number) => ThemeShadowMap>

export function deriveThemeDimensionMap(
  seedTokens: ResolvedThemeSeedTokens,
  componentOverrides: ThemeComponentOverrides | undefined,
  preset: ThemePresetId,
): ThemeDimensionMap {
  const fontSize = seedTokens.fontSize
  const fontSizeXs = Math.max(9, fontSize - 2)
  const fontSizeSm = Math.max(10, fontSize - 1)
  const fontSizeLg = fontSize + 1
  const fontSizeXl = fontSize + 2
  const controlHeight = seedTokens.controlHeight
  const controlHeightStep = Math.max(2, Math.round(seedTokens.sizeStep * 1.5))
  const tableRowHeight = componentOverrides?.tableRowHeight ?? DEFAULT_DIMENSIONS.tableRowHeight
  const borderRadius = seedTokens.borderRadius
  const radiusStep = Math.max(1, Math.round(seedTokens.sizeStep / 2))
  const cardBorderRadius = componentOverrides?.cardBorderRadius
    ?? DEFAULT_DIMENSIONS.cardBorderRadius

  return {
    fontSizeXs: `${fontSizeXs}px`,
    fontSizeSm: `${fontSizeSm}px`,
    fontSize: `${fontSize}px`,
    fontSizeLg: `${fontSizeLg}px`,
    fontSizeXl: `${fontSizeXl}px`,
    lineHeightXs: `${fontSizeXs + 5}px`,
    lineHeightSm: `${fontSizeSm + 6}px`,
    lineHeight: `${fontSize + 7}px`,
    lineHeightLg: `${fontSizeLg + 8}px`,
    lineHeightXl: `${fontSizeXl + 9}px`,
    sizeUnit: `${seedTokens.sizeUnit}px`,
    sizeStep: `${seedTokens.sizeStep}px`,
    controlHeightSm: `${Math.max(24, controlHeight - controlHeightStep)}px`,
    controlHeight: `${controlHeight}px`,
    controlHeightLg: `${controlHeight + controlHeightStep}px`,
    tableRowHeight: `${tableRowHeight}px`,
    tableRowHeightSm: `${Math.max(32, tableRowHeight - 20)}px`,
    borderRadiusSm: `${Math.max(0, borderRadius - radiusStep)}px`,
    borderRadius: `${borderRadius}px`,
    borderRadiusLg: `${borderRadius + seedTokens.sizeStep}px`,
    cardBorderRadius: `${cardBorderRadius}px`,
    layoutSiderBorderRadius: preset === 'soft' ? `${borderRadius + seedTokens.sizeStep * 3}px` : '0px',
  }
}

export function deriveThemeShadowMap(
  theme: ThemeName,
  shadowStrength: number,
): ThemeShadowMap {
  return SHADOW_MAP_DERIVERS[theme](shadowStrength)
}

function deriveDarkShadowMap(shadowStrength: number): ThemeShadowMap {
  return {
    boxShadow: scaleShadowAlpha(`0 24px 52px -26px ${withAlpha(BLACK, 0.82)}`, shadowStrength),
    boxShadowSecondary: scaleShadowAlpha(`0 18px 34px -26px ${withAlpha(BLACK, 0.64)}`, shadowStrength),
    boxShadowTertiary: scaleShadowAlpha(`0 12px 24px -20px ${withAlpha(BLACK, 0.66)}`, shadowStrength),
  }
}

function deriveLightShadowMap(shadowStrength: number): ThemeShadowMap {
  return {
    boxShadow: scaleShadowAlpha(`0 18px 38px -18px ${withAlpha(LIGHT_SHADOW_BASE, 0.17)}`, shadowStrength),
    boxShadowSecondary: scaleShadowAlpha(`0 10px 22px -18px ${withAlpha(LIGHT_SHADOW_BASE, 0.08)}`, shadowStrength),
    boxShadowTertiary: scaleShadowAlpha(`0 9px 18px -14px ${withAlpha(LIGHT_SHADOW_BASE, 0.086)}`, shadowStrength),
  }
}

export function deriveThemeComponentMap(
  surfaces: ThemeSurfaceMap,
  fills: ThemeFillMap | null,
  primary: ThemePrimaryMap,
  error: FunctionalColorMap,
  shadowStrength: number,
  preset: ThemePresetId,
): ThemeComponentMap {
  // 明暗模式共用组件合同，各自保留适合宿主表面的填充配方。
  const components = fills
    ? deriveLightComponentMap(surfaces, fills, primary, error, shadowStrength)
    : deriveDarkComponentMap(surfaces, primary, error, shadowStrength)
  const buttonPrimaryBg = ensureContrast(primary.colorPrimary, primary.colorTextLightSolid, 4.5)
  // 关闭态复用主题的中性轮廓色，与卡片区分但不额外加深。
  const switchUncheckedBg = surfaces.colorBorder
  return {
    ...components,
    ...(preset === 'soft' ? deriveSoftSurfaceMap(surfaces, shadowStrength, fills === null) : {}),
    switchUncheckedBg,
    switchUncheckedColor: ensureContrast(surfaces.colorTextSecondary, switchUncheckedBg, 4.5),
    // 暗色选中行只轻微提亮，保留原有蓝灰色温和选中语义。
    tableRowSelectedHoverBg: fills
      ? primary.colorPrimaryContainerHover
      : mix(primary.colorPrimaryContainer, surfaces.colorText, 0.04),
    buttonPrimaryColor: primary.colorTextLightSolid,
    buttonPrimaryBg,
    buttonPrimaryHoverBg: mix(buttonPrimaryBg, BLACK, 0.08),
    buttonPrimaryActiveBg: mix(buttonPrimaryBg, BLACK, 0.16),
  }
}

function deriveDarkComponentMap(
  surfaces: ThemeSurfaceMap,
  primary: ThemePrimaryMap,
  error: FunctionalColorMap,
  shadowStrength: number,
): SurfaceComponentMap {
  const inputInteractionBg = mix(surfaces.colorBgContainer, surfaces.colorFillSecondary, 0.69)
  // 悬停与聚焦共用反馈；装饰阴影关闭时仍保留可见外圈。
  const inputInteractionShadow = [
    `0 0 0 3px ${primary.colorPrimaryContainerHover}`,
    shadowStrength > 0
      ? scaleShadowAlpha(`0 14px 28px -22px ${withAlpha(BLACK, 0.72)}`, shadowStrength)
      : null,
  ].filter(Boolean).join(', ')

  return {
    buttonSecondaryBg: surfaces.colorFillTertiary,
    buttonSecondaryHoverBg: surfaces.colorBgTextActive,
    buttonSecondaryActiveBg: surfaces.colorBgTextActive,
    iconButtonSecondaryBg: surfaces.colorBgContainer,
    iconButtonSecondaryHoverBg: surfaces.colorBgTextHover,
    iconButtonSecondaryActiveBg: surfaces.colorBgTextActive,
    menuItemSelectedBg: surfaces.colorBgTextHover,
    menuItemSelectedBgImage: 'none',
    menuItemSelectedShadow: 'none',
    inputBg: mix(surfaces.colorBgContainer, surfaces.colorFillSecondary, 0.375),
    inputHoverBg: inputInteractionBg,
    inputActiveBg: inputInteractionBg,
    inputErrorActiveBg: error.container,
    brandMarkBg: surfaces.colorBgElevated,
    cardBg: surfaces.colorBgContainer,
    cardBgImage: 'none',
    modalBg: surfaces.colorBgContainer,
    popoverHeaderBg: surfaces.colorFillTertiary,
    tableHeaderBg: surfaces.colorFillTertiary,
    tableRowBg: surfaces.colorBgContainer,
    tableRowStripeBg: surfaces.colorFillQuaternary,
    tableRowHoverBg: surfaces.colorBgTextHover,
    tableRowSelectedBg: primary.colorPrimaryContainer,
    progressRemainingColor: mix(surfaces.colorBgContainer, surfaces.colorBorderSecondary, 0.92),
    layoutSiderBg: surfaces.colorBgContainer,
    layoutSiderBgImage: 'none',
    cardShadow: scaleShadowAlpha(`0 18px 34px -26px ${withAlpha(BLACK, 0.64)}`, shadowStrength),
    popoverShadow: scaleShadowAlpha(`0 1px 6px -1px ${withAlpha(BLACK, 0.30)}, 0 10px 22px -11px ${withAlpha(BLACK, 0.52)}`, shadowStrength),
    inputShadow: scaleShadowAlpha(`0 12px 24px -20px ${withAlpha(BLACK, 0.66)}`, shadowStrength),
    inputHoverShadow: inputInteractionShadow,
    inputActiveShadow: inputInteractionShadow,
    inputErrorActiveShadow: `0 0 0 3px ${withAlpha(error.color, 0.28)}`,
    layoutSiderShadow: scaleShadowAlpha(`2px 0 18px -14px ${withAlpha(BLACK, 0.72)}`, shadowStrength),
    scrollbarThumbBg: mix(surfaces.colorBorderSecondary, surfaces.colorTextSecondary, 0.16),
    scrollbarThumbHoverBg: mix(surfaces.colorBorderSecondary, surfaces.colorTextSecondary, 0.3),
  }
}

function deriveLightComponentMap(
  surfaces: ThemeSurfaceMap,
  fills: ThemeFillMap,
  primary: ThemePrimaryMap,
  error: FunctionalColorMap,
  shadowStrength: number,
): SurfaceComponentMap {
  const inputInteractionShadow = `0 0 0 3px ${withAlpha(primary.colorPrimary, 0.16)}`
  return {
    buttonSecondaryBg: fills.controlBg,
    buttonSecondaryHoverBg: fills.controlHoverBg,
    buttonSecondaryActiveBg: fills.controlActiveBg,
    iconButtonSecondaryBg: fills.controlBg,
    iconButtonSecondaryHoverBg: fills.controlHoverBg,
    iconButtonSecondaryActiveBg: fills.controlActiveBg,
    inputBg: fills.controlBg,
    inputHoverBg: surfaces.colorBgContainer,
    inputActiveBg: surfaces.colorBgContainer,
    inputErrorActiveBg: error.container,
    inputShadow: 'none',
    inputHoverShadow: inputInteractionShadow,
    inputActiveShadow: inputInteractionShadow,
    inputErrorActiveShadow: `0 0 0 3px ${withAlpha(error.color, 0.18)}`,
    // 先合成实色，固定列滚动时不会透出下层内容。
    tableHeaderBg: compositeOnBackground(fills.contentBg, surfaces.colorBgContainer),
    tableRowBg: surfaces.colorBgContainer,
    tableRowStripeBg: compositeOnBackground(fills.contentStripeBg, surfaces.colorBgContainer),
    tableRowHoverBg: compositeOnBackground(fills.contentHoverBg, surfaces.colorBgContainer),
    tableRowSelectedBg: primary.colorPrimaryContainer,
    menuItemSelectedBg: surfaces.colorBgTextActive,
    menuItemSelectedBgImage: 'none',
    menuItemSelectedShadow: 'none',
    brandMarkBg: surfaces.colorBgSpotlight,
    cardBg: surfaces.colorBgContainer,
    cardBgImage: 'none',
    modalBg: surfaces.colorBgContainer,
    popoverHeaderBg: surfaces.colorFillSecondary,
    progressRemainingColor: surfaces.colorBorderSecondary,
    layoutSiderBg: surfaces.colorBgContainer,
    layoutSiderBgImage: 'none',
    cardShadow: scaleShadowAlpha(`0 10px 22px -18px ${withAlpha(LIGHT_SHADOW_BASE, 0.08)}`, shadowStrength),
    // 面板与容器同色时，短阴影也能分开上沿和两侧。
    popoverShadow: scaleShadowAlpha(`0 1px 6px -1px ${withAlpha(LIGHT_SHADOW_BASE, 0.09)}, 0 8px 20px -10px ${withAlpha(LIGHT_SHADOW_BASE, 0.08)}`, shadowStrength),
    layoutSiderShadow: scaleShadowAlpha(`2px 0 12px -12px ${withAlpha(LIGHT_SHADOW_BASE, 0.027)}`, shadowStrength),
    scrollbarThumbBg: mix(surfaces.colorBgLayout, surfaces.colorTextSecondary, 0.2),
    scrollbarThumbHoverBg: mix(surfaces.colorBgLayout, surfaces.colorTextSecondary, 0.34),
  }
}

function deriveSoftSurfaceMap(
  surfaces: ThemeSurfaceMap,
  shadowStrength: number,
  dark: boolean,
): Partial<SurfaceComponentMap> {
  const highlight = dark
    ? mix(surfaces.colorBgContainer, surfaces.colorBgElevated, 0.4)
    : mix(surfaces.colorBgContainer, WHITE, 0.12)
  const surfaceBgImage = dark
    ? `linear-gradient(145deg, ${highlight}, transparent 75%)`
    : `linear-gradient(145deg, ${highlight}, transparent)`
  return {
    cardBgImage: surfaceBgImage,
    // 暗色通过大范围低对比的明暗过渡表现体积，避免亮边形成金属倒角
    cardShadow: scaleShadowAlpha(dark
      ? `inset 0 3px 10px ${withAlpha(WHITE, 0.035)}, inset 0 -2px 8px ${withAlpha(BLACK, 0.14)}, -3px -3px 12px -4px ${withAlpha(WHITE, 0.018)}, 0 8px 18px -12px ${withAlpha(BLACK, 0.42)}`
      : `inset 0 2px 1px ${withAlpha(highlight, 0.92)}, inset 0 -2px 3px ${withAlpha(LIGHT_SHADOW_BASE, 0.055)}, 0 3px 5px -3px ${withAlpha(LIGHT_SHADOW_BASE, 0.16)}, 0 16px 28px -20px ${withAlpha(LIGHT_SHADOW_BASE, 0.28)}`, shadowStrength),
    layoutSiderBgImage: dark
      ? surfaceBgImage
      : `linear-gradient(270deg, ${withAlpha(LIGHT_SHADOW_BASE, 0.018)}, ${withAlpha(highlight, 0.65)} 3px, transparent 10px), linear-gradient(160deg, ${highlight}, transparent)`,
    layoutSiderShadow: scaleShadowAlpha(dark
      ? `inset 0 3px 10px ${withAlpha(WHITE, 0.025)}, inset -2px 0 8px ${withAlpha(BLACK, 0.12)}, 4px 0 16px -12px ${withAlpha(BLACK, 0.42)}`
      : `inset -3px 0 5px -4px ${withAlpha(highlight, 0.8)}, 1px 0 6px -4px ${withAlpha(LIGHT_SHADOW_BASE, 0.06)}, 4px 0 12px -8px ${withAlpha(LIGHT_SHADOW_BASE, 0.08)}`, shadowStrength),
    menuItemSelectedBgImage: `linear-gradient(145deg, ${dark ? surfaces.colorBgElevated : withAlpha(highlight, 0.12)}, transparent)`,
    menuItemSelectedShadow: scaleShadowAlpha(dark
      ? `inset 0 2px 6px ${withAlpha(WHITE, 0.035)}, inset 0 -1px 4px ${withAlpha(BLACK, 0.12)}, 0 3px 8px -5px ${withAlpha(BLACK, 0.35)}`
      : `inset 0 2px 1px ${withAlpha(highlight, 0.9)}, inset 0 -1px 1px ${withAlpha(LIGHT_SHADOW_BASE, 0.07)}, 0 3px 6px -4px ${withAlpha(LIGHT_SHADOW_BASE, 0.3)}`, shadowStrength),
  }
}
