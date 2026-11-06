<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { appInfo, hideToTray, type AppInfo } from '../ipc'

const info = ref<AppInfo | null>(null)
const error = ref<string | null>(null)

onMounted(async () => {
  try {
    info.value = await appInfo()
  } catch (e) {
    error.value = String(e)
  }
})

async function onHide() {
  try {
    await hideToTray()
  } catch (e) {
    error.value = String(e)
  }
}
</script>

<template>
  <section>
    <h1>Settings</h1>
    <p class="page-description">Startup behavior, tray options, and information about this build.</p>

    <dl v-if="info" class="info">
      <dt>Name</dt>
      <dd>{{ info.name }}</dd>
      <dt>Version</dt>
      <dd>{{ info.version }}</dd>
      <dt>Build profile</dt>
      <dd>{{ info.profile }}</dd>
    </dl>
    <p v-if="error" class="error">{{ error }}</p>

    <button type="button" @click="onHide">Hide to tray</button>
  </section>
</template>

<style scoped>
.info {
  display: grid;
  grid-template-columns: max-content 1fr;
  gap: var(--space-2) var(--space-6);
  margin: var(--space-8) 0;
}

.info dt {
  color: var(--color-text-muted);
}

.info dd {
  margin: 0;
}

.error {
  margin: var(--space-6) 0;
  color: var(--color-text-muted);
}
</style>
