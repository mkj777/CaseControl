<script setup lang="ts">
// Conflicts: other software that may hold the same devices (ADR 0001 #12, #31). Per finding: what was found, how
// it starts with Windows with Turn off / Re-enable, End process / Stop service behind a confirmation, and for RGB
// suites "Don't ask again" for the start dialog. Every action that needs administrator rights runs behind at most
// one Windows permission prompt; nothing changes without a click.
import { computed, onMounted, ref } from 'vue'
import {
  Button,
  Card,
  ConfirmDialog,
  EmptyState,
  KeyValueList,
  Notice,
  PageHeader,
  StatusBadge,
  Switch,
} from '../components/ui'
import {
  closeProgram,
  errorText,
  scanConflicts,
  setAutostart,
  setDontAsk,
  type ActionReport,
  type CloseTarget,
  type Finding,
} from '../ipc'
import {
  SEVERITY_LABEL,
  actionText,
  autostartText,
  closeTargetKey,
  closeTargetLabel,
  evidenceText,
  groupBySeverity,
} from '../lib/conflicts'
import { familyLabel } from '../lib/device'
import { conflictState, takeScan } from '../stores/conflicts'

const scan = computed(() => conflictState.scan)
const scanning = ref(false)
const error = ref<string | null>(null)
/** Outcome of the last action: lines and whether everything worked, or some steps are not confirmed yet. */
const outcome = ref<{ state: 'ok' | 'unknown' | 'failed'; lines: string[] } | null>(null)

async function runScan() {
  scanning.value = true
  error.value = null
  try {
    takeScan(await scanConflicts())
  } catch (e) {
    error.value = errorText(e)
  } finally {
    scanning.value = false
  }
}

onMounted(runScan)

const affectsText = (f: Finding) =>
  f.affects.includes('all') ? 'All device families' : f.affects.map(familyLabel).join(', ')

// ---- actions -----------------------------------------------------------------------------------------------------

const acting = ref(false)

function show(report: ActionReport) {
  takeScan(report.scan)
  const lines = [...report.results.map(actionText), ...report.refused.map((r) => r.reason)]
  const failed = report.results.some((r) => !r.ok && !r.unknown) || report.refused.length > 0
  const unknown = report.results.some((r) => r.unknown)
  outcome.value = {
    state: failed ? 'failed' : unknown ? 'unknown' : 'ok',
    lines: lines.length > 0 ? lines : ['Nothing needed to change.'],
  }
}

async function act(run: () => Promise<ActionReport>) {
  acting.value = true
  error.value = null
  outcome.value = null
  try {
    show(await run())
  } catch (e) {
    error.value = errorText(e)
  } finally {
    acting.value = false
  }
}

const pending = ref<{ finding: Finding; target: CloseTarget } | null>(null)

async function onConfirmClose() {
  const p = pending.value
  if (!p) return
  await act(() => closeProgram(p.finding.entry_id, [p.target]))
  pending.value = null
}

const toggleAutostart = (f: Finding, enabled: boolean) => act(() => setAutostart(f.entry_id, enabled))

const needsAdmin = (f: Finding, enabled: boolean) =>
  f.autostart.some((i) => i.toggleable && i.enabled !== enabled && i.needs_admin)

async function onDontAsk(f: Finding, value: boolean) {
  error.value = null
  try {
    takeScan(await setDontAsk(f.entry_id, value))
  } catch (e) {
    error.value = errorText(e)
  }
}
</script>

<template>
  <section class="page">
    <PageHeader title="Conflicts" description="Other software that can drive the same devices, so colours flicker or fall back.">
      <template #actions>
        <Button :disabled="scanning" @click="runScan">{{ scanning ? 'Scanning…' : 'Scan' }}</Button>
      </template>
    </PageHeader>

    <Notice v-if="error" variant="error">{{ error }}</Notice>
    <Notice v-if="outcome" :variant="outcome.state === 'failed' ? 'error' : outcome.state === 'unknown' ? 'warning' : 'info'">
      {{
        outcome.state === 'failed'
          ? 'Some steps did not work.'
          : outcome.state === 'unknown'
            ? 'Some steps are not confirmed yet. Scan again in a moment.'
            : 'Done.'
      }}
      <template #details>
        <p v-for="(line, i) in outcome.lines" :key="i" class="selectable">{{ line }}</p>
      </template>
    </Notice>
    <Notice v-if="scan && scan.errors.length > 0" variant="warning">
      Parts of the scan failed, so the list may be incomplete:
      <template #details>
        <p v-for="(e, i) in scan.errors" :key="i" class="selectable">{{ e }}</p>
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
                <li v-for="(e, i) in f.evidence" :key="i" class="selectable">{{ evidenceText(e) }}</li>
              </ul>
            </dd>
            <template v-if="f.autostart.length > 0">
              <dt>Starts with Windows</dt>
              <dd>
                <ul>
                  <li v-for="(item, i) in f.autostart" :key="i" class="selectable">
                    {{ autostartText(item) }}
                    <span v-if="item.command" class="command">{{ item.command }}</span>
                  </li>
                </ul>
              </dd>
            </template>
          </KeyValueList>

          <div v-if="f.close_targets.length > 0 || f.autostart_on || f.can_reenable" class="row">
            <Button
              v-for="t in f.close_targets"
              :key="closeTargetKey(t)"
              variant="danger"
              :disabled="acting"
              @click="pending = { finding: f, target: t }"
            >
              {{ closeTargetLabel(t) }}
            </Button>
            <Button v-if="f.autostart_on" :disabled="acting" @click="toggleAutostart(f, false)">
              Turn off autostart
            </Button>
            <Button v-if="f.can_reenable" variant="ghost" :disabled="acting" @click="toggleAutostart(f, true)">
              Re-enable autostart
            </Button>
          </div>
          <p v-if="(f.autostart_on && needsAdmin(f, false)) || (f.can_reenable && needsAdmin(f, true))" class="muted small">
            Changing autostart here shows a Windows permission prompt.
          </p>

          <Switch
            v-if="f.class === 'suite'"
            :model-value="f.dont_ask"
            label="Don't ask again at start"
            @update:model-value="(v: boolean) => onDontAsk(f, v)"
          />
        </Card>
      </div>
    </template>
    <p v-else-if="scanning" class="muted">Scanning…</p>

    <ConfirmDialog
      :open="pending !== null"
      :title="pending?.target.kind === 'process' ? 'End process?' : 'Stop service?'"
      :confirm-label="pending?.target.kind === 'process' ? 'End process' : 'Stop service'"
      danger
      :busy="acting"
      @confirm="onConfirmClose"
      @cancel="pending = null"
    >
      <template v-if="pending?.target.kind === 'process'">
        <p>
          CaseControl will end {{ pending.target.name }} (process {{ pending.target.pid }}) of {{ pending.finding.name }}.
          Unsaved work in it is lost. If Windows does not allow it, a permission prompt appears.
        </p>
        <p>It may start again the next time you sign in unless you turn off its autostart.</p>
      </template>
      <template v-else-if="pending?.target.kind === 'service'">
        <p>
          Windows will ask for administrator permission, then CaseControl stops the service {{ pending.target.name }} of
          {{ pending.finding.name }}.
        </p>
        <p>It starts again with the next restart unless you turn off its autostart.</p>
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

.command {
  display: block;
  font-family: var(--font-mono);
  color: var(--text-muted);
  overflow-wrap: anywhere;
}
</style>
