import type {
  ThemeColorId,
  ThemeColorPreset,
  ThemeColorPresetId,
  ThemeComponentOverrides,
  ThemeCustomization,
  ThemeMode,
  ThemeName,
  ThemePreset,
  ThemePresetColorName,
  ThemePresetId,
  ThemeSeedColors,
  ThemeSeedDimensions,
  ThemeTokenName,
} from '../types'
import { presetPrimaryColors } from '@ant-design/colors'

export const DEFAULT_THEME_MODE: ThemeMode = 'system'
export const DEFAULT_THEME_COLOR: ThemeColorPresetId = 'relay-blue'
export const DEFAULT_CUSTOM_THEME_COLOR = '#5983F4'
export const DEFAULT_THEME_CUSTOMIZATION: ThemeCustomization = {}
export const defaultThemePresetId: ThemePresetId = 'default'

export const themePresets: readonly ThemePreset[] = [
  { id: 'default', colorId: DEFAULT_THEME_COLOR, primaryColor: DEFAULT_CUSTOM_THEME_COLOR },
  { id: 'soft', colorId: DEFAULT_THEME_COLOR, primaryColor: DEFAULT_CUSTOM_THEME_COLOR },
]

export const BLACK = '#000000'
export const WHITE = '#FFFFFF'
export const LIGHT_FOREGROUND = WHITE
export const LIGHT_CONTAINER_BASE = WHITE
export const LIGHT_TEXT_BASE = '#0E1726'
export const DARK_CONTAINER_BASE = '#111A29'
export const DARK_TEXT_BASE = '#D8E2EE'

export const PRESET_SURFACE_BACKGROUNDS: Partial<Record<ThemeColorId, Record<ThemeName, string>>> = {
  'deep-teal': { light: '#F8F8F3', dark: '#202727' },
  'signal-violet': { light: '#FCF6F7', dark: '#29242A' },
  'graphite': { light: '#FBF5E8', dark: '#302421' },
}

const lightSeedColors = {
  colorSuccess: '#12B981',
  colorWarning: '#F59E0B',
  colorError: '#EF4444',
  colorInfo: DEFAULT_CUSTOM_THEME_COLOR,
  colorLink: DEFAULT_CUSTOM_THEME_COLOR,
  colorTextBase: LIGHT_TEXT_BASE,
  colorBgBase: LIGHT_CONTAINER_BASE,
} satisfies ThemeSeedColors

export const SEED_COLORS_BY_SCHEME = {
  light: lightSeedColors,
  dark: { ...lightSeedColors, colorTextBase: DARK_TEXT_BASE, colorBgBase: DARK_CONTAINER_BASE },
} satisfies Record<ThemeName, ThemeSeedColors>

export const THEME_SEED_COLOR_KEYS = Object.keys(lightSeedColors) as (keyof ThemeSeedColors)[]

export const PRESET_COLOR_SEEDS = {
  blue: presetPrimaryColors.blue,
  cyan: presetPrimaryColors.cyan,
  green: presetPrimaryColors.green,
  orange: presetPrimaryColors.orange,
  purple: presetPrimaryColors.purple,
  red: presetPrimaryColors.red,
} as const satisfies Record<ThemePresetColorName, string>

export const DEFAULT_SEED_DIMENSIONS = {
  fontSize: 13,
  sizeUnit: 4,
  sizeStep: 4,
  controlHeight: 38,
  borderRadius: 8,
  shadowStrength: 100,
} satisfies ThemeSeedDimensions

export const DEFAULT_COMPONENT_DIMENSIONS = {
  tableRowHeight: 60,
  cardBorderRadius: 18,
} satisfies Required<ThemeComponentOverrides>

export const THEME_COLOR_PRESETS: readonly ThemeColorPreset[] = [
  {
    id: 'relay-blue',
    label: '星津夜航',
    description: '星垂海面，帆过深蓝',
    seed: DEFAULT_CUSTOM_THEME_COLOR,
  },
  {
    id: 'deep-teal',
    label: '水墨江汀',
    description: '远山淡墨，一叶横舟',
    seed: '#50605E',
  },
  {
    id: 'signal-violet',
    label: '粉墙回廊',
    description: '粉墙留光，长廊留白',
    seed: '#D395A8',
  },
  {
    id: 'graphite',
    label: '朱阙金瓦',
    description: '金瓦出檐，朱柱映阶',
    seed: '#A34537',
  },
] as const

export const themeProfiles: Readonly<Record<ThemeColorPresetId, { preset: ThemePresetId, customization: ThemeCustomization }>> = {
  'relay-blue': { preset: 'default', customization: {} },
  'deep-teal': {
    preset: 'default',
    customization: {
      seed: { controlHeight: 36, borderRadius: 4, shadowStrength: 55 },
      component: { cardBorderRadius: 6, tableRowHeight: 60 },
    },
  },
  'signal-violet': {
    preset: 'default',
    customization: {
      seed: { fontSize: 14, controlHeight: 42, borderRadius: 5 },
      component: { cardBorderRadius: 8, tableRowHeight: 68 },
    },
  },
  'graphite': {
    preset: 'soft',
    customization: {
      seed: { controlHeight: 38, borderRadius: 6, shadowStrength: 80 },
      component: { cardBorderRadius: 12, tableRowHeight: 54 },
    },
  },
}

/** 中继蓝浅色主题的稳定中性色阶；带色温主题会连续过渡到通用派生结果。 */
export const LIGHT_SURFACE_ANCHORS = {
  colorBgSpotlight: '#111827',
  colorFillSecondary: '#E9EEF5',
  colorFillTertiary: '#F1F5F9',
  colorFillQuaternary: '#F6F8FB',
  colorBorder: '#D8E0EA',
  colorBorderSecondary: '#E2E8F0',
  colorTextHeading: '#111827',
  colorTextSecondary: '#64748B',
  colorTextMuted: '#94A3B8',
} as const

/** 中继蓝暗色主题的视觉锚点；带色温主题会连续过渡到通用派生结果。 */
export const DARK_SURFACE_ANCHORS = {
  colorBgLayout: '#0B111C',
  colorBgMask: '#030712',
  colorBgSpotlight: '#050913',
} as const

/** 浅色阴影保持中性，不随主题文字色染色。 */
export const LIGHT_SHADOW_BASE = LIGHT_TEXT_BASE

export const EDITABLE_COLOR_TOKEN_NAMES = new Set<ThemeTokenName>([
  '--cp-menu-item-selected-bg',
  '--cp-input-bg',
  '--cp-input-hover-bg',
  '--cp-input-active-bg',
  '--cp-button-primary-color',
  '--cp-button-primary-bg',
  '--cp-button-primary-hover-bg',
  '--cp-button-primary-active-bg',
  '--cp-button-secondary-bg',
  '--cp-button-secondary-hover-bg',
  '--cp-button-secondary-active-bg',
  '--cp-icon-button-secondary-bg',
  '--cp-icon-button-secondary-hover-bg',
  '--cp-icon-button-secondary-active-bg',
  '--cp-card-bg',
  '--cp-modal-bg',
  '--cp-popover-header-bg',
  '--cp-table-header-bg',
  '--cp-table-row-bg',
  '--cp-table-row-stripe-bg',
  '--cp-table-row-hover-bg',
  '--cp-table-row-selected-bg',
  '--cp-table-row-selected-hover-bg',
  '--cp-progress-remaining-color',
  '--cp-layout-sider-bg',
  '--cp-scrollbar-thumb-bg',
  '--cp-scrollbar-thumb-hover-bg',
])

export const EDITABLE_SHADOW_TOKEN_NAMES = new Set<ThemeTokenName>([
  '--cp-input-shadow',
  '--cp-input-hover-shadow',
  '--cp-input-active-shadow',
  '--cp-card-shadow',
  '--cp-layout-sider-shadow',
])

/** 只有控件填充允许透出宿主背景；容器与固定表格单元格保持实色。 */
export const ALPHA_COLOR_TOKEN_NAMES = new Set<string>([
  '--cp-input-bg',
  '--cp-input-hover-bg',
  '--cp-input-active-bg',
  '--cp-button-secondary-bg',
  '--cp-button-secondary-hover-bg',
  '--cp-button-secondary-active-bg',
  '--cp-icon-button-secondary-bg',
  '--cp-icon-button-secondary-hover-bg',
  '--cp-icon-button-secondary-active-bg',
] satisfies ThemeTokenName[])
