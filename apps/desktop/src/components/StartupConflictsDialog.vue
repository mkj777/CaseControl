<script setup lang="ts">
// The start dialog (ADR 0001 #31): RGB suites that control one of the user's devices, with "Close now" and "Turn
// off autostart" (both preselected) and "Don't ask again" per program. Nothing happens until Proceed; Not now (also
// Esc) only stores the "Don't ask again" ticks. After Proceed the dialog shows what happened per action.
import { computed, reactive, ref, watch } from 'vue'
import { Button, Dialog, Notice } from './ui'
import { errorText, resolveConflicts, setDontAsk, type ActionReport, type ConflictProgram } from '../ipc'
import { actionText } from '../lib/conflicts'
import { closeDialog, conflictState, takeScan } from '../stores/conflicts'

interface Choice {
  close: boolean
  autostart: boolean
  dontAsk: boolean
}

const choices = reactive<Record<string, Choice>>({})
const phase = ref<'choose' | 'busy' | 'done'>('choose')
const report = ref<ActionReport | null>(null)
const error = ref<string | null>(null)
/** The programs as they were when the dialog opened (the live list changes after Proceed). */
const shown = ref<ConflictProgram[]>([])

watch(
  () => conflictState.dialogOpen,
  (open) => {
    if (!open) return
    shown.value = [...conflictState.programs]
    for (const key of Object.keys(choices)) delete choices[key]
    for (const p of shown.value) {
      choices[p.entry_id] = {
        close: p.close_targets.length > 0,
        autostart: p.autostart.length > 0,
        dontAsk: false,
      }
    }
    phase.value = 'choose'
    report.value = null
    error.value = null
  },
  { immediate: true },
)

const devicesText = (p: ConflictProgram) => p.affected_devices.map((d) => d.name).join(', ')

const anySelected = computed(() =>
  shown.value.some((p) => choices[p.entry_id]?.close || choices[p.entry_id]?.autostart),
)

/** A permission prompt is possible: a service to stop or an all-users / service autostart item is selected. */
const mayPrompt = computed(() =>
  shown.value.some((p) => {
    const c = choices[p.entry_id]
    if (!c) return false
    return (
      (c.close && p.close_targets.some((t) => t.kind === 'service')) ||
      (c.autostart && p.autostart.some((i) => i.needs_admin))
    )
  }),
)

async function saveDontAsk() {
  for (const p of shown.value) {
    if (choices[p.entry_id]?.dontAsk) takeScan(await setDontAsk(p.entry_id, true))
  }
}

async function notNow() {
  if (phase.value === 'busy') return
  if (phase.value === 'choose') {
    try {
      await saveDontAsk()
    } catch (e) {
      error.value = errorText(e)
      return
    }
  }
  closeDialog()
}

async function proceed() {
  phase.value = 'busy'
  error.value = null
  try {
    await saveDontAsk()
    const items = shown.value
      .map((p) => ({
        entry_id: p.entry_id,
        close: choices[p.entry_id]?.close ?? false,
        disable_autostart: choices[p.entry_id]?.autostart ?? false,
      }))
      .filter((i) => i.close || i.disable_autostart)
    if (items.length > 0) {
      report.value = await resolveConflicts({ items })
      takeScan(report.value.scan)
      phase.value = 'done'
    } else {
      closeDialog()
    }
  } catch (e) {
    error.value = errorText(e)
    phase.value = 'choose'
  }
}

const nameOf = (id: string) => shown.value.find((p) => p.entry_id === id)?.name ?? id
const unknown = computed(() => report.value?.results.some((r) => r.unknown) ?? false)
const failed = computed(() => (report.value?.results.filter((r) => !r.ok && !r.unknown).length ?? 0) + (report.value?.refused.length ?? 0))
</script>

<template>
  <Dialog
    :open="conflictState.dialogOpen"
    :title="phase === 'done' ? 'Other RGB programs' : 'Close other RGB programs?'"
    :busy="phase === 'busy'"
    @close="notNow"
  >
    <template v-if="phase !== 'done'">
      <p>
        These programs control the same devices as CaseControl, so colours flicker or get overwritten. CaseControl will
        close them and stop them from starting with Windows. You can turn autostart back on from the Conflicts page.
      </p>

      <div v-for="p in shown" :key="p.entry_id" class="program">
        <div class="name">{{ p.name }}</div>
        <div class="small">Controls {{ devicesText(p) }}</div>
        <div v-if="choices[p.entry_id]" class="options">
          <label v-if="p.close_targets.length > 0" class="option">
            <input v-model="choices[p.entry_id]!.close" type="checkbox" :disabled="phase === 'busy'" />
            Close now
          </label>
          <span v-else class="small">Not running</span>
          <label v-if="p.autostart.length > 0" class="option">
            <input v-model="choices[p.entry_id]!.autostart" type="checkbox" :disabled="phase === 'busy'" />
            Turn off autostart
          </label>
          <label class="option">
            <input v-model="choices[p.entry_id]!.dontAsk" type="checkbox" :disabled="phase === 'busy'" />
            Don't ask again
          </label>
        </div>
      </div>

      <p v-if="mayPrompt && anySelected" class="small">Windows may ask for permission once.</p>
      <Notice v-if="error" variant="error">{{ error }}</Notice>
    </template>

    <template v-else-if="report">
      <p v-if="failed > 0">Some steps did not work. The programs may still interfere with your devices.</p>
      <p v-else-if="unknown">Windows is still finishing some steps. The Conflicts page shows the result after a scan.</p>
      <p v-else>Done. The programs are closed as chosen.</p>
      <ul class="results selectable">
        <li v-for="(r, i) in report.results" :key="i" :class="{ failed: !r.ok && !r.unknown }">
          {{ nameOf(r.entry_id) }}: {{ actionText(r) }}
        </li>
        <li v-for="r in report.refused" :key="`refused-${r.entry_id}`" class="failed">
          {{ nameOf(r.entry_id) }}: {{ r.reason }}
        </li>
      </ul>
    </template>

    <template #actions>
      <template v-if="phase !== 'done'">
        <Button data-autofocus :disabled="phase === 'busy'" @click="notNow">Not now</Button>
        <Button variant="primary" :disabled="phase === 'busy' || !anySelected" @click="proceed">
          {{ phase === 'busy' ? 'Closing…' : 'Proceed' }}
        </Button>
      </template>
      <Button v-else variant="primary" data-autofocus @click="closeDialog">Done</Button>
    </template>
  </Dialog>
</template>

<style scoped>
.program {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  padding-top: var(--space-3);
  border-top: 1px solid var(--border);
}

.name {
  font-weight: var(--weight-strong);
  color: var(--text);
}

.options {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2) var(--space-5);
  margin-top: var(--space-2);
}

.option {
  display: inline-flex;
  align-items: center;
  gap: var(--space-2);
  min-height: 24px;
  color: var(--text);
}

.results {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  padding-left: var(--space-5);
}

.results .failed {
  color: var(--error);
}
</style>
