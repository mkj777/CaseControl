// Live updates while dragging (ADR 0001 #21): send at most once per animation frame and never more than one
// request at a time; whatever arrives meanwhile is coalesced into the latest value. The engine coalesces further
// down to the configured frame rate. Because every value goes through one queue in order, the last value pushed is
// always the last one sent.

export interface LatestSender<T> {
  /** Queue `value`; earlier queued values that were not sent yet are dropped. */
  push(value: T): void
  /** Drop a queued value that was not sent yet (a request already in flight still completes). */
  cancel(): void
  /** Send the queued value now (skipping the frame wait) and resolve when everything queued has been sent. */
  flush(): Promise<void>
  /** Nothing queued and nothing in flight. */
  idle(): boolean
}

export function latestSender<T>(
  send: (value: T) => Promise<unknown>,
  onError: (e: unknown) => void,
  onIdle: () => void = () => {},
): LatestSender<T> {
  let queued: { value: T } | null = null
  let scheduled = false
  let inFlight: Promise<void> | null = null

  const pump = (): Promise<void> => {
    if (inFlight || !queued) return inFlight ?? Promise.resolve()
    const { value } = queued
    queued = null
    inFlight = send(value)
      .then(
        () => undefined,
        (e) => onError(e),
      )
      .finally(() => {
        inFlight = null
        if (queued && !scheduled) void pump()
        else if (!queued) onIdle()
      })
    return inFlight
  }

  return {
    push(value) {
      queued = { value }
      if (scheduled) return
      scheduled = true
      requestAnimationFrame(() => {
        scheduled = false
        void pump()
      })
    },
    cancel() {
      queued = null
    },
    async flush() {
      while (queued || inFlight) {
        if (inFlight) await inFlight
        else await pump()
      }
    },
    idle: () => !queued && !inFlight,
  }
}
