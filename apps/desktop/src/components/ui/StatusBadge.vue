<script setup lang="ts">
// Status pill (docs/design-system.md, "Status badge"). The label in the slot carries the meaning; colour is only
// for the states that need attention: Faulted (red) and a running conflict (amber).
//   active       device under control             released     device in its own hardware mode
//   hidden       hidden device or zone            faulted      device stopped after an error
//   running      conflicting software is running  not-running  conflicting software installed or not running
//   current      the active profile
export type BadgeStatus = 'active' | 'released' | 'hidden' | 'faulted' | 'running' | 'not-running' | 'current'

defineProps<{ status: BadgeStatus }>()
</script>

<template>
  <span class="badge" :class="status"><slot /></span>
</template>

<style scoped>
.badge {
  display: inline-block;
  flex: none;
  padding: 2px var(--space-2);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-full);
  font-size: var(--text-sm);
  font-weight: var(--weight-regular);
  line-height: var(--leading);
  white-space: nowrap;
  color: var(--text-muted);
}

.active {
  color: var(--text);
}

.hidden {
  border-style: dashed;
  color: var(--text-faint);
}

.faulted {
  border-color: var(--status-faulted);
  color: var(--status-faulted);
}

.running {
  border-color: var(--warning);
  color: var(--warning);
}

.current {
  border-color: var(--text);
  color: var(--text);
}
</style>
