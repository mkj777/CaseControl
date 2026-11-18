# Brief: `crates/cc-engine` (Phase 2)

Read `00-common-rules.md` first. You own `crates/cc-engine/**`. Architecture section "Engine" is the spec; this brief
fixes the API the desktop app will call. Drivers are being written at the same time; build and test entirely against
fake `Backend`/`Controller` implementations of your own (`cc_core` traits), and take real backends as a constructor
argument (`cc_drivers::all_backends()` is what the app passes).

## API

```rust
pub struct EngineConfig {
    pub frame_rate: u32,                                   // 1..=60
    pub disabled_families: HashSet<String>,
    pub zone_sizes: BTreeMap<DeviceId, BTreeMap<String, usize>>, // by zone name, applied right after detection
    pub hotplug: bool,                                     // false in tests
}
pub fn start(cfg: EngineConfig, backends: Vec<Box<dyn Backend>>) -> Result<EngineHandle, EngineError>;

#[derive(Clone)] pub struct EngineHandle { .. }            // Send + Sync, cheap to clone
impl EngineHandle {
    pub fn devices(&self) -> Vec<DeviceView>;              // sorted by kind, then name
    pub fn device(&self, id: &DeviceId) -> Option<DeviceView>;
    pub fn set_direct(&self, id: &DeviceId, colors: Vec<Rgb>) -> Result<(), EngineError>;          // coalesced
    pub fn set_zone(&self, id: &DeviceId, zone: usize, colors: Vec<Rgb>) -> Result<(), EngineError>; // coalesced
    pub fn set_mode(&self, id: &DeviceId, state: ModeState) -> Result<(), EngineError>;            // acked
    pub fn resize_zone(&self, id: &DeviceId, zone: usize, len: usize) -> Result<(), EngineError>;  // acked
    pub fn save_to_hardware(&self, id: &DeviceId) -> Result<(), EngineError>;                      // acked
    pub fn release(&self, id: &DeviceId) -> Result<(), EngineError>;                               // acked
    pub fn release_all(&self) -> Vec<(DeviceId, Result<(), EngineError>)>;
    pub fn apply(&self, settings: &BTreeMap<DeviceId, DeviceSetting>) -> ApplyReport;  // applied / missing / failed
    pub fn current_settings(&self) -> BTreeMap<DeviceId, DeviceSetting>;               // for "save as profile"
    pub fn set_frame_rate(&self, fps: u32);
    pub fn set_disabled_families(&self, families: HashSet<String>);  // disabling releases and drops those devices
    pub fn rescan(&self);                                  // detect again with open ids in ctx.skip
    pub fn subscribe(&self) -> crossbeam_channel::Receiver<EngineEvent>;
    pub fn shutdown(&self, timeout: Duration);             // idempotent
}
pub struct DeviceView { pub description: DeviceDescription, pub status: DeviceStatus,
                        pub setting: Option<DeviceSetting>, pub colors: Vec<Rgb> }   // serde::Serialize
pub enum DeviceStatus { Active, Released, Faulted { error: String } }                // serde, tag = "state"
```

`DeviceSetting` is `cc_config::DeviceSetting`. Add `serde`, `serde_json` (dev) and anything else to your Cargo.toml.

## Behaviour

- Detection runs inside `start` (synchronously, so `devices()` is complete when `start` returns). Each `Detected::Device`
  gets a worker thread (named `cc-dev-<id>`) owning the `Box<dyn Controller>` and a bounded command channel.
  `Detected::Failed` is logged and kept in a `failures()` list (add that method) for diagnostics.
- Zone sizes from `cfg.zone_sizes` are applied by name right after detection via `resize_zone`.
- Colour edits: the engine keeps each device's full colour buffer. `set_zone` writes into the buffer; both set the
  device's setting to `Direct` and mark it dirty. The device thread sends at most one `set_direct` per frame interval
  (1/frame_rate) with the latest buffer (latest wins, nothing queued up). Validate lengths against the description
  before accepting (`EngineError::InvalidArgument`). Calls after `release` re-take control (drivers support that).
- Acked commands wait for the device thread's result with a 3 s timeout (`EngineError::Timeout`).
- Keepalive: if `description.keepalive` is `Some(d)`, call `keepalive()` every `d` while the device is Active.
- Error policy exactly as architecture: back-off 50 ms doubling to 5 s on write errors, `Faulted` after 10 consecutive
  failures or on any error with `is_fatal()`, never a tight loop. A faulted device's thread stops talking to it.
- Hot-plug (`cfg.hotplug`): one `cc_hid::hotplug::watch`. On `Arrived`, run `detect` of every `Bus::Hid` backend with
  all open ids in `ctx.skip`; new devices start like at startup and get the pending setting for their id (see apply).
  On `Removed`, the device whose `info.location` equals the path (case-insensitive) is dropped without `release`
  (it is gone) and `DeviceRemoved` is emitted.
- `apply`: for each entry, fit the setting to the device (`DeviceSetting::fit_to` from cc-config if it exists when you
  build; otherwise implement the documented pad/truncate rule locally and say so), then `Direct` -> `set_direct`,
  `Mode` -> `set_mode`. Entries for absent devices are remembered as pending and applied when that id appears.
- Events: `EngineEvent::DeviceAdded/Removed/Faulted/StateChanged` from `cc-core` on a broadcast to every subscriber
  (drop disconnected subscribers). Emit `StateChanged` for mode changes; colour frames do not emit events.
- Shutdown: release every Active device (each with its own share of the timeout), stop hot-plug, join threads, emit
  nothing afterwards. Make `Drop` of the last handle call `shutdown` with 2 s if it was not called.

## Tests (fakes only)

Detection to views; coalescing (100 `set_direct` calls in a burst produce at most a few frames, last one wins);
`set_zone` updates only its slice; acked command results and timeout; back-off and Faulted after 10 failures (use a
clock you can inject or short real durations); fatal error; keepalive cadence; apply with present and pending devices;
`set_disabled_families` releases; shutdown releases all in order and is idempotent; hot-plug arrival/removal by
injecting events (make the hot-plug source injectable instead of calling `cc_hid` in tests).
