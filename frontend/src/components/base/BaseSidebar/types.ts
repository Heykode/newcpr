import type { Component } from 'vue'
import type Sidebar from './Sidebar.vue'

export interface SidebarChildItem {
  index: string
  label: string
  icon?: Component
  disabled?: boolean
}

export interface SidebarItem extends SidebarChildItem {
  children?: SidebarChildItem[]
}

export type SidebarProps = InstanceType<typeof Sidebar>['$props']
export type SidebarEmits = InstanceType<typeof Sidebar>['$emit']
export type SidebarSlots = InstanceType<typeof Sidebar>['$slots']
export type SidebarInstance = InstanceType<typeof Sidebar>
