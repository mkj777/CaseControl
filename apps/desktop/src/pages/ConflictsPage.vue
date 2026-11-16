<script setup lang="ts">
// Conflicts: other software that may hold the same devices, with opt-in actions behind a confirmation
// (ADR 0001 #12). Nothing here changes how that software starts.
import { onBeforeUnmount, onMounted, ref } from 'vue'
import {
  Button,
  Card,
  ConfirmDialog,
  EmptyState,
  KeyValueList,
  Notice,
  PageHeader,
  StatusBadge,
} from '../components/ui'
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
    <PageHeader
      title="Conflicts"
      description="Other software that can control the same devices. When two programs drive a device, colours flicker or fall back. CaseControl never changes how this software starts."
    >
      <template #actions>
        <Button :disabled="scanning" @click="runScan">{{ scanning ? 'Scanning…' : 'Scan' }}</Button>
      </template>
    </PageHeader>

    <Notice v-if="error" variant="error">{{ error }}</Notice>
    <Notice v-if="message">{{ message }}</Notice>
    <Notice v-if="scan && scan.errors.length > 0" variant="warning">
      Parts of the scan failed, so the list may be incomplete:
      <template #details>
        <p v-for="(e, i) in scan.errors" :key="i">{{ e }}</p>
      </template>
    </Notice>

    <template v-if="scan">
      <EmptyState v-if="scan.findings.length === 0" title="Nothing found">
        No other software competes for your devices.
      </EmptyState>

      <div v-for="group in groupBySeverity(scan.findings)" :key="group.severity" class="group">
        <h2>{{ SEVERITY_LABEL[group.severity] }}</h2>
        <Card v-for="f in group.findings" :key="f.entry_id" as="article">
          <div class="finding-head">
            <div>
              <div class="name">{{ f.name }}</div>
              <div class="muted small">{{ f.vendor }}</div>
            </div>
            <StatusBadge :status="f.running ? 'running' : 'not-running'">
              {{ f.running ? 'Running' : f.installed ? 'Installed, not running' : 'Not running' }}
            </StatusBadge>
          </div>

          <p v-if="f.note" class="note">{{ f.note }}</p>

          <KeyValueList class="small">
            <dt>Affects</dt>
            <dd>
              <template v-if="f.affected_devices.length > 0">
                {{ f.affected_devices.map((d) => d.name).join(', ') }}
              </template>
              <template v-else>None of your detected devices ({{ affectsText(f) }})</template>
            </dd>
            <dt>Found</dt>
            <dd>
              <ul>
                <li v-for="(e, i) in f.evidence" :key="i">{{ evidenceText(e) }}</li>
              </ul>
            </dd>
          </KeyValueList>

          <div v-if="actionsOf(f).length > 0" class="row">
            <Button
              v-for="a in actionsOf(f)"
              :key="a.kind === 'process' ? `p${a.pid}` : `s${a.name}`"
              variant="danger"
              :disabled="acting"
              @click="pending = { finding: f, action: a }"
            >
              {{ a.kind === 'process' ? `End process ${a.name}` : `Stop ${a.driver ? 'driver' : 'service'} ${a.name}` }}
            </Button>
          </div>
        </Card>
      </div>
    </template>
    <p v-else-if="scanning" class="muted">Scanning…</p>

    <ConfirmDialog
      :open="pending !== null"
      :title="pending?.action.kind === 'process' ? 'End process?' : 'Stop service?'"
      :confirm-label="pending?.action.kind === 'process' ? 'End process' : 'Stop'"
      danger
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
.group + .group {
  margin-top: var(--space-14);
}

.finding-head {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: var(--space-4);
}

.name {
  font-size: var(--text-lg);
  font-weight: var(--weight-strong);
  line-height: var(--leading-tight);
}

.note {
  max-width: var(--measure);
  color: var(--text-muted);
}
</style>
