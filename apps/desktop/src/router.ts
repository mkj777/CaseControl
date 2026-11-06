import { createRouter, createWebHashHistory, type RouteRecordRaw } from 'vue-router'
import DevicesPage from './pages/DevicesPage.vue'
import ProfilesPage from './pages/ProfilesPage.vue'
import ConflictsPage from './pages/ConflictsPage.vue'
import SettingsPage from './pages/SettingsPage.vue'

/** The four top-level pages (ADR 0001 #14), in sidebar order. */
export const routes: RouteRecordRaw[] = [
  { path: '/', redirect: '/devices' },
  { path: '/devices', name: 'devices', component: DevicesPage },
  { path: '/profiles', name: 'profiles', component: ProfilesPage },
  { path: '/conflicts', name: 'conflicts', component: ConflictsPage },
  { path: '/settings', name: 'settings', component: SettingsPage },
]

// Hash history: the app is served from Tauri's custom protocol, which has no server-side fallback.
export const router = createRouter({
  history: createWebHashHistory(),
  routes,
})
