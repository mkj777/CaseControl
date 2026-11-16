<script setup lang="ts">
// A button that is only a glyph (docs/design-system.md, "Icon button"), for actions every Windows user reads from
// the glyph alone: window buttons, close in a dialog, back. `label` is required and becomes the accessible name and
// the native tooltip. `glyph` is a Segoe Fluent Icons character. `window` sizes it as a title bar button.
withDefaults(defineProps<{ label: string; glyph: string; window?: boolean; disabled?: boolean }>(), {
  window: false,
  disabled: false,
})
</script>

<template>
  <button type="button" class="icon-button" :class="{ window }" :aria-label="label" :title="label" :disabled="disabled">
    <span class="glyph" aria-hidden="true">{{ glyph }}</span>
  </button>
</template>

<style scoped>
.icon-button {
  width: var(--icon-button-size);
  height: var(--icon-button-size);
  padding: 0;
  border: 0;
  border-radius: var(--radius-md);
  color: var(--text-muted);
  background: none;
}

.icon-button:hover:not(:disabled) {
  color: var(--text);
  background: var(--surface-hover);
}

.icon-button:active:not(:disabled) {
  background: var(--surface-pressed);
}

.glyph {
  font-family: var(--font-icon);
  font-size: 12px;
  line-height: 1;
}

/* Title bar button: full strip height, square corners, focus ring kept inside the window edge. */
.window {
  width: var(--window-button-width);
  height: var(--titlebar-height);
  border-radius: 0;
}

.window .glyph {
  font-size: 10px;
}

.window:focus-visible {
  outline-offset: -2px;
}
</style>
