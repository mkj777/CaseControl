import { createRouter, createWebHashHistory, type RouteRecordRaw } from 'vue-router'
import LightingPage from './pages/LightingPage.vue'
import DevicePage from './pages/DevicePage.vue'
import ConflictsPage from './pages/ConflictsPage.vue'
import SettingsPage from './pages/SettingsPage.vue'

/** The three top-level pages (ADR 0001 #14, #37, #40), in sidebar order, plus the device view (ADR 0001 #18). The
 * Lighting page was called Devices until ADR 0001 #37; its old paths redirect. Profiles are managed in the Lighting
 * toolbar since ADR 0001 #40. */
export const routes: RouteRecordRaw[] = [
  { path: '/', redirect: '/lighting' },
  { path: '/lighting', name: 'lighting', component: LightingPage },
  { path: '/lighting/:id', name: 'device', component: DevicePage, props: true },
  { path: '/devices', redirect: '/lighting' },
  { path: '/devices/:id', redirect: (to) => ({ name: 'device', params: { id: to.params.id } }) },
  { path: '/conflicts', name: 'conflicts', component: ConflictsPage },
  { path: '/settings', name: 'settings', component: SettingsPage },
]

// Hash history: the app is served from Tauri's custom protocol, which has no server-side fallback.
export const router = createRouter({
  history: createWebHashHistory(),
  routes,
})
