<script setup lang="ts">
// Confirmation before an action (docs/design-system.md, "Dialog"), built on Dialog. The title is a question, the
// confirm label repeats the action, the body slot says what happens and what cannot be undone. Cancel has the
// initial focus. `danger` makes the confirm button the danger variant (never primary for a danger action).
import Button from './Button.vue'
import Dialog from './Dialog.vue'

withDefaults(
  defineProps<{ open: boolean; title: string; confirmLabel?: string; busy?: boolean; danger?: boolean }>(),
  { confirmLabel: 'Confirm', busy: false, danger: false },
)
const emit = defineEmits<{ confirm: []; cancel: [] }>()
</script>

<template>
  <Dialog :open="open" :title="title" role="alertdialog" :busy="busy" @close="emit('cancel')">
    <slot />
    <template #actions>
      <Button data-autofocus :disabled="busy" @click="emit('cancel')">Cancel</Button>
      <Button :variant="danger ? 'danger' : 'primary'" :disabled="busy" @click="emit('confirm')">
        {{ confirmLabel }}
      </Button>
    </template>
  </Dialog>
</template>
