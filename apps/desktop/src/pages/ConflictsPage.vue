<script setup lang="ts">
// Conflicts: other software that may hold the same devices, with opt-in actions behind a confirmation
// (ADR 0001 #12). Nothing here changes how that software starts.
import { onBeforeUnmount, onMounted, ref } from 'vue'
import ConfirmDialog from '../components/ConfirmDialog.vue'
import { errorText, on, scanConflicts, stopService, terminateProcess, type ConflictScan, type Finding } from '../ipc'
import { SEVERITY_LABEL, actionsOf, evidenceText, groupBySeverity, type FindingAction } from '../lib/conflicts'
import { familyLabel } from '../lib/device'

const scan = ref<ConflictScan | null>(null)
const scanning = ref(false)
const error = ref<string | null>(null)
const message = ref<string | null>(null)

async function runScan() {
  scanning.value = true
  error.value = null
  try {
    scan.value = await scanConflicts()
  } catch (e) {
    error.value = errorText(e)
  } finally {
    scanning.value = false
  }
}

let unlisten: (() => void) | null = null
onMounted(async () => {
  unlisten = await on('conflicts-updated', (s) => (scan.value = s))
  await runScan()
})
onBeforeUnmount(() => unlisten?.())

const affectsText = (f: Finding) =>
  f.affects.includes('all') ? 'All device families' : f.affects.map(familyLabel).join(', ')

// ---- actions -----------------------------------------------------------------------------------------------------

const pending = ref<{ finding: Finding; action: FindingAction } | null>(null)
const acting = ref(false)

async function onConfirm() {
  const p = pending.value
  if (!p) return
  acting.value = true
  error.value = null
  message.value = null
  try {
    if (p.action.kind === 'process') {
      await terminateProcess(p.action.pid, p.finding.entry_id)
      message.value = `${p.action.name} was ended.`
      await runScan()
    } else {
      await stopService(p.action.name, p.finding.entry_id)
      message.value = `Windows was asked to stop ${p.action.name}. Scan again once you have answered the prompt.`
    }
  } catch (e) {
    error.value = errorText(e)
  } finally {
    acting.value = false
    pending.value = null
  }
}
</script>

<template>
  <section class="page">
    <header class="page-header">
      <div>
        <h1>Conflicts</h1>
        <p class="page-description">
          Other software that can control the same devices. When two programs drive a device, colours flicker or
          fall back. CaseControl never changes how this software starts.
        </p>
      </div>
      <button type="button" :disabled="scanning" @click="runScan">{{ scanning ? 'Scanning…' : 'Scan' }}</button>
    </header>

    <p v-if="error" class="notice">{{ error }}</p>
    <p v-if="message" class="notice">{{ message }}</p>
    <div v-if="scan && scan.errors.length > 0" class="notice">
      <p>Parts of the scan failed, so the list may be incomplete:</p>
      <p v-for="(e, i) in scan.errors" :key="i" class="muted small">{{ e }}</p>
    </div>

    <template v-if="scan">
      <p v-if="scan.findings.length === 0" class="empty muted">Nothing found that competes for your devices.</p>

      <div v-for="group in groupBySeverity(scan.findings)" :key="group.severity" class="group">
        <h2>{{ SEVERITY_LABEL[group.severity] }}</h2>
        <article v-for="f in group.findings" :key="f.entry_id" class="finding">
          <div class="finding-head">
            <div>
              <div class="name">{{ f.name }}</div>
              <div class="muted small">{{ f.vendor }}</div>
            </div>
            <span class="state small" :class="{ running: f.running }">
              {{ f.running ? 'Running' : f.installed ? 'Installed, not running' : 'Not running' }}
            </span>
          </div>

          <p v-if="f.note" class="note">{{ f.note }}</p>

          <dl class="facts small">
            <dt>Affects</dt>
            <dd>
              <template v-if="f.affected_devices.length > 0">
                {{ f.affected_devices.map((d) => d.name).join(', ') }}
              </template>
              <template v-else>None of your detected devices ({{ affectsText(f) }})</template>
            </dd>
            <dt>Found</dt>
            <dd>
              <ul class="evidence">
                <li v-for="(e, i) in f.evidence" :key="i">{{ evidenceText(e) }}</li>
              </ul>
            </dd>
          </dl>

          <div v-if="actionsOf(f).length > 0" class="row">
            <button
              v-for="a in actionsOf(f)"
              :key="a.kind === 'process' ? `p${a.pid}` : `s${a.name}`"
              type="button"
              :disabled="acting"
              @click="pending = { finding: f, action: a }"
            >
              {{ a.kind === 'process' ? `End process ${a.name}` : `Stop ${a.driver ? 'driver' : 'service'} ${a.name}` }}
            </button>
          </div>
        </article>
      </div>
    </template>
    <p v-else-if="scanning" class="empty muted">Scanning…</p>

    <ConfirmDialog
      :open="pending !== null"
      :title="pending?.action.kind === 'process' ? 'End process?' : 'Stop service?'"
      :confirm-label="pending?.action.kind === 'process' ? 'End process' : 'Stop'"
      :busy="acting"
      @confirm="onConfirm"
      @cancel="pending = null"
    >
      <template v-if="pending?.action.kind === 'process'">
        <p>
          CaseControl will end {{ pending.action.name }} (process {{ pending.action.pid }}) of {{ pending.finding.name }}.
          Unsaved work in it is lost.
        </p>
        <p>It may start again the next time you sign in; its startup setting is not changed.</p>
      </template>
      <template v-else-if="pending?.action.kind === 'service'">
        <p>
          Windows will ask for administrator permission, then stop the {{ pending.action.driver ? 'driver' : 'service' }}
          {{ pending.action.name }} of {{ pending.finding.name }}.
        </p>
        <p>It starts again with the next restart; its startup setting is not changed.</p>
      </template>
    </ConfirmDialog>
  </section>
</template>

<style scoped>
.notice {
  margin-bottom: var(--space-6);
}

.group {
  margin-bottom: var(--space-10);
}

.finding {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
  padding: var(--space-6);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-lg);
  background: var(--color-surface);
}

.finding + .finding {
  margin-top: var(--space-4);
}

.finding-head {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: var(--space-4);
}

.name {
  font-size: var(--font-size-lg);
  font-weight: 600;
}

.state {
  flex: none;
  padding: 2px var(--space-2);
  border: 1px solid var(--color-border-strong);
  border-radius: 999px;
  color: var(--color-text-muted);
}

.state.running {
  border-color: var(--color-text);
  color: var(--color-text);
}

.note {
  max-width: 70ch;
  color: var(--color-text-muted);
}

.facts {
  display: grid;
  grid-template-columns: max-content 1fr;
  gap: var(--space-2) var(--space-6);
  margin: 0;
}

.facts dt {
  color: var(--color-text-muted);
}

.facts dd {
  margin: 0;
}

.evidence {
  margin: 0;
  padding: 0;
  list-style: none;
}

.empty {
  padding: var(--space-8) 0;
}
</style>
