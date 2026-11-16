import { createApp } from 'vue'
import App from './App.vue'
import { router } from './router'
import './styles.css'

// The browser's own context menu does not belong in a desktop app; keep it only where text is edited (copy/paste).
// In-app menus (hide/unhide) open from their own `contextmenu` handlers.
const TEXT_INPUT =
  'textarea, [contenteditable]:not([contenteditable="false"]), ' +
  'input:not([type]), input[type="text"], input[type="number"], input[type="search"]'

document.addEventListener('contextmenu', (e) => {
  const target = e.target instanceof Element ? e.target : null
  if (!target?.closest(TEXT_INPUT)) e.preventDefault()
})

createApp(App).use(router).mount('#app')
