<script setup lang="ts">
// A brightness slider (ADR 0001 #33): a labelled Slider with a percent readout. While dragging it sends live
// (coalesced, `persist = false`); when the drag ends it sends the final value once more with `persist = true`. Until
// that last send has finished it shows its own value, so a device refresh in between cannot pull the thumb back.
import { computed, ref } from 'vue'
import { latestSender } from '../lib/throttle'
import { Field, Slider } from './ui'

const props = withDefaults(
  defineProps<{
    label: string
    /** The current value in percent. */
    value: number
    /** Sends a value; with `persist` it is the final one and should be saved. Rejects with the error. */
    send: (percent: number, persist: boolean) => Promise<unknown>
    disabled?: boolean
    hint?: string
  }>(),
  { disabled: false, hint: undefined },
)
const emit = defineEmits<{ error: [error: unknown] }>()

const draft = ref<number | null>(null)
const shown = computed(() => draft.value ?? props.value)

const live = latestSender<number>(
  (v) => props.send(v, false),
  (e) => emit('error', e),
)

function onInput(v: number) {
  draft.value = v
  live.push(v)
}

async function onChange(v: number) {
  draft.value = v
  // The final value replaces whatever live value is still queued.
  live.cancel()
  try {
    await live.flush()
    await props.send(v, true)
  } catch (e) {
    emit('error', e)
  } finally {
    draft.value = null
  }
}
</script>

<template>
  <Field :label="label" :hint="hint">
    <template #default="{ id, describedBy }">
      <Slider
        :id="id"
        :aria-describedby="describedBy"
        :model-value="shown"
        :min="0"
        :max="100"
        :disabled="disabled"
        @update:model-value="onInput"
        @change="onChange"
      />
    </template>
    <template #value>{{ shown }} %</template>
  </Field>
</template>
