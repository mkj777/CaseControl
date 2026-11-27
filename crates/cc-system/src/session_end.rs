//! Why the app is being closed: sign-out, shutdown / restart, a Restart Manager close (installer) or a normal quit
//! (ADR 0001 #42).
//!
//! Tauri's exit event carries no reason, and tao answers `WM_ENDSESSION` by exiting the process without passing the
//! `lParam` on. [`install`] therefore subclasses the app's main window and tao's thread event-target window (class
//! `Tao Thread Event Target`, found among the calling thread's top-level windows) with one subclass procedure that
//! records the `lParam` flags of `WM_QUERYENDSESSION` and `WM_ENDSESSION` in atomics and always passes the message
//! on (`DefSubclassProc`). Subclassing tao's window means the flags are recorded before tao calls
//! `std::process::exit`. [`current`] combines the recorded messages with `GetSystemMetrics(SM_SHUTTINGDOWN)`; the
//! decoding is the pure [`classify`].
//!
//! Whether the session ends is decided only by `WM_ENDSESSION` with `wParam` TRUE or by `SM_SHUTTINGDOWN`. A
//! `WM_QUERYENDSESSION` alone proves nothing (another app may cancel the session end, and then no `WM_ENDSESSION`
//! TRUE follows); its flags only tell the kind, and only for [`QUERY_TTL`] after it arrived. A `WM_ENDSESSION` FALSE
//! (cancelled) forgets everything recorded before it.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::Error;

/// `ENDSESSION_CLOSEAPP`: the Restart Manager closes the app (an installer), the session does not end.
pub const ENDSESSION_CLOSEAPP: u32 = 0x0000_0001;
/// `ENDSESSION_CRITICAL`: forced; informational only.
pub const ENDSESSION_CRITICAL: u32 = 0x4000_0000;
/// `ENDSESSION_LOGOFF`: the user signs out.
pub const ENDSESSION_LOGOFF: u32 = 0x8000_0000;

/// Why the app is being closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionEnd {
    /// The session is not ending: a normal quit.
    NotEnding,
    /// The user signs out (`ENDSESSION_LOGOFF`).
    SignOut,
    /// Windows shuts down or restarts (session end without the sign-out flag; the two cannot be told apart).
    ShutdownOrRestart,
    /// The Restart Manager closes the app (`ENDSESSION_CLOSEAPP`, typically the installer or uninstaller).
    CloseApp,
    /// The session is ending (`SM_SHUTTINGDOWN`) but no message was seen, so the kind is not known.
    Unknown,
}

/// A session message as the subclass procedure recorded it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMessage {
    /// `WM_QUERYENDSESSION` with its `lParam` flags.
    Query {
        /// `ENDSESSION_*` flags.
        flags: u32,
    },
    /// `WM_ENDSESSION`: `ending` is `wParam` (FALSE when the session end was cancelled), with the `lParam` flags.
    End {
        /// Whether the session really ends.
        ending: bool,
        /// `ENDSESSION_*` flags.
        flags: u32,
    },
}

const SEEN: u64 = 1 << 63;
const IS_END: u64 = 1 << 62;
const ENDING: u64 = 1 << 61;

impl SessionMessage {
    /// Packs the message into one `u64` (never 0) for an atomic.
    pub fn encode(self) -> u64 {
        match self {
            SessionMessage::Query { flags } => SEEN | u64::from(flags),
            SessionMessage::End { ending, flags } => {
                SEEN | IS_END | if ending { ENDING } else { 0 } | u64::from(flags)
            }
        }
    }

    /// Unpacks [`SessionMessage::encode`]; 0 is "nothing seen".
    pub fn decode(raw: u64) -> Option<Self> {
        if raw & SEEN == 0 {
            return None;
        }
        let flags = (raw & 0xFFFF_FFFF) as u32;
        Some(if raw & IS_END != 0 {
            SessionMessage::End {
                ending: raw & ENDING != 0,
                flags,
            }
        } else {
            SessionMessage::Query { flags }
        })
    }
}

/// How long the flags of a `WM_QUERYENDSESSION` are used for the kind of a session end.
pub const QUERY_TTL: Duration = Duration::from_secs(30);

/// What the subclass procedure recorded, as [`classify`] reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Recorded {
    /// The flags of the last `WM_QUERYENDSESSION` and how long ago it arrived.
    pub query: Option<(u32, Duration)>,
    /// The flags of a `WM_ENDSESSION` with `wParam` TRUE (the session really ends).
    pub ended: Option<u32>,
}

fn kind(flags: u32) -> SessionEnd {
    if flags & ENDSESSION_CLOSEAPP != 0 {
        SessionEnd::CloseApp
    } else if flags & ENDSESSION_LOGOFF != 0 {
        SessionEnd::SignOut
    } else {
        SessionEnd::ShutdownOrRestart
    }
}

/// Pure decoding of what was recorded and whether `SM_SHUTTINGDOWN` is set.
///
/// - The session ends only when a `WM_ENDSESSION` TRUE was seen or `SM_SHUTTINGDOWN` is set; otherwise
///   [`SessionEnd::NotEnding`], whatever `WM_QUERYENDSESSION` said.
/// - The kind comes from the `WM_ENDSESSION` TRUE flags, else from a `WM_QUERYENDSESSION` younger than
///   [`QUERY_TTL`], else it is [`SessionEnd::Unknown`].
/// - Kind: `CLOSEAPP` in the flags: [`SessionEnd::CloseApp`]; else `LOGOFF`: [`SessionEnd::SignOut`]; else
///   [`SessionEnd::ShutdownOrRestart`].
pub fn classify(recorded: Recorded, shutting_down: bool) -> SessionEnd {
    if let Some(flags) = recorded.ended {
        return kind(flags);
    }
    if !shutting_down {
        return SessionEnd::NotEnding;
    }
    match recorded.query {
        Some((flags, age)) if age <= QUERY_TTL => kind(flags),
        _ => SessionEnd::Unknown,
    }
}

/// The last `WM_QUERYENDSESSION` flags | [`SEEN`]; 0 = none.
static QUERY: AtomicU64 = AtomicU64::new(0);
/// When it arrived, in milliseconds since [`epoch`].
static QUERY_AT: AtomicU64 = AtomicU64::new(0);
/// The flags of a `WM_ENDSESSION` TRUE | [`SEEN`]; 0 = none.
static ENDED: AtomicU64 = AtomicU64::new(0);

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn millis_since_epoch() -> u64 {
    u64::try_from(epoch().elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Records a session message (called by the subclass procedure).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn record(message: SessionMessage) {
    match message {
        SessionMessage::Query { flags } => {
            QUERY_AT.store(millis_since_epoch(), Ordering::SeqCst);
            QUERY.store(SEEN | u64::from(flags), Ordering::SeqCst);
        }
        SessionMessage::End { ending: true, flags } => {
            ENDED.store(SEEN | u64::from(flags), Ordering::SeqCst);
        }
        SessionMessage::End { ending: false, .. } => {
            // Cancelled: forget the query that announced it.
            QUERY.store(0, Ordering::SeqCst);
            ENDED.store(0, Ordering::SeqCst);
        }
    }
    tracing::info!(?message, "session message");
}

/// What was recorded since start (for [`classify`] and logging).
pub fn last_message() -> Recorded {
    let query = QUERY.load(Ordering::SeqCst);
    let ended = ENDED.load(Ordering::SeqCst);
    Recorded {
        query: (query & SEEN != 0).then(|| {
            let at = QUERY_AT.load(Ordering::SeqCst);
            let age = Duration::from_millis(millis_since_epoch().saturating_sub(at));
            ((query & 0xFFFF_FFFF) as u32, age)
        }),
        ended: (ended & SEEN != 0).then_some((ended & 0xFFFF_FFFF) as u32),
    }
}

/// Why the app is being closed right now (call it from the exit handler).
pub fn current() -> SessionEnd {
    classify(last_message(), shutting_down())
}

/// `GetSystemMetrics(SM_SHUTTINGDOWN)`: the session is ending (sign-out or shutdown). `false` off Windows.
pub fn shutting_down() -> bool {
    #[cfg(windows)]
    {
        crate::sys::shutting_down()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Which windows [`install`] subclassed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Installed {
    /// The window passed in.
    pub main_window: bool,
    /// tao's thread event-target window (`false` when none was found on this thread).
    pub tao_target: bool,
}

/// Subclasses `hwnd` (a top-level window of the calling thread, as `isize`) and tao's thread event-target window of
/// the calling thread so session messages are recorded. Call it on the thread that owns the window (the main /
/// event-loop thread). Calling it again is harmless. Fails when `hwnd` is not a window of this thread.
pub fn install(hwnd: isize) -> Result<Installed, Error> {
    #[cfg(windows)]
    {
        crate::sys::install_session_hooks(hwnd)
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        Err(Error::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_round_trips() {
        for m in [
            SessionMessage::Query { flags: 0 },
            SessionMessage::Query {
                flags: ENDSESSION_LOGOFF | ENDSESSION_CRITICAL,
            },
            SessionMessage::End {
                ending: true,
                flags: ENDSESSION_CLOSEAPP,
            },
            SessionMessage::End {
                ending: false,
                flags: 0,
            },
        ] {
            assert_ne!(m.encode(), 0);
            assert_eq!(SessionMessage::decode(m.encode()), Some(m));
        }
        assert_eq!(SessionMessage::decode(0), None);
    }

    fn query(flags: u32, secs: u64) -> Recorded {
        Recorded {
            query: Some((flags, Duration::from_secs(secs))),
            ended: None,
        }
    }

    fn ended(flags: u32) -> Recorded {
        Recorded {
            query: None,
            ended: Some(flags),
        }
    }

    #[test]
    fn classification() {
        let none = Recorded::default();
        let cases = [
            (none, false, SessionEnd::NotEnding),
            (none, true, SessionEnd::Unknown),
            (query(0, 1), true, SessionEnd::ShutdownOrRestart),
            (ended(0), true, SessionEnd::ShutdownOrRestart),
            (ended(0), false, SessionEnd::ShutdownOrRestart),
            (query(ENDSESSION_LOGOFF, 1), true, SessionEnd::SignOut),
            (
                ended(ENDSESSION_LOGOFF | ENDSESSION_CRITICAL),
                true,
                SessionEnd::SignOut,
            ),
            (query(ENDSESSION_CRITICAL, 1), true, SessionEnd::ShutdownOrRestart),
            (ended(ENDSESSION_CLOSEAPP), false, SessionEnd::CloseApp),
            (
                ended(ENDSESSION_CLOSEAPP | ENDSESSION_LOGOFF),
                false,
                SessionEnd::CloseApp,
            ),
            // The WM_ENDSESSION flags win over an older query.
            (
                Recorded {
                    query: Some((ENDSESSION_LOGOFF, Duration::from_secs(1))),
                    ended: Some(ENDSESSION_CLOSEAPP),
                },
                false,
                SessionEnd::CloseApp,
            ),
        ];
        for (recorded, shutting, want) in cases {
            assert_eq!(classify(recorded, shutting), want, "{recorded:?} {shutting}");
        }
    }

    /// A WM_QUERYENDSESSION alone (no WM_ENDSESSION TRUE followed, e.g. another app cancelled) never makes a later
    /// normal quit a session end, however fresh it is and whatever its flags.
    #[test]
    fn a_query_alone_is_not_a_session_end() {
        for flags in [0, ENDSESSION_LOGOFF, ENDSESSION_CLOSEAPP, ENDSESSION_CRITICAL] {
            for secs in [0, 5, 3600] {
                assert_eq!(
                    classify(query(flags, secs), false),
                    SessionEnd::NotEnding,
                    "{flags:#x} {secs}"
                );
            }
        }
    }

    /// A stale query does not decide the kind of a later shutdown.
    #[test]
    fn query_flags_expire() {
        assert_eq!(
            classify(query(ENDSESSION_LOGOFF, QUERY_TTL.as_secs()), true),
            SessionEnd::SignOut
        );
        assert_eq!(
            classify(query(ENDSESSION_LOGOFF, QUERY_TTL.as_secs() + 1), true),
            SessionEnd::Unknown
        );
    }

    /// The recording sequence: a query, a cancel (forgets it), a query and the real end. One test so the shared
    /// statics are not raced by another test.
    #[test]
    fn recording_sequences() {
        record(SessionMessage::Query {
            flags: ENDSESSION_LOGOFF,
        });
        let r = last_message();
        assert_eq!(r.query.map(|q| q.0), Some(ENDSESSION_LOGOFF));
        assert!(r.query.is_some_and(|q| q.1 < QUERY_TTL));
        assert_eq!(r.ended, None);
        assert_eq!(classify(r, false), SessionEnd::NotEnding);
        record(SessionMessage::End {
            ending: false,
            flags: ENDSESSION_LOGOFF,
        });
        assert_eq!(last_message(), Recorded::default());
        assert_eq!(classify(last_message(), true), SessionEnd::Unknown);
        record(SessionMessage::Query {
            flags: ENDSESSION_CLOSEAPP,
        });
        record(SessionMessage::End {
            ending: true,
            flags: ENDSESSION_CLOSEAPP,
        });
        assert_eq!(classify(last_message(), false), SessionEnd::CloseApp);
        QUERY.store(0, Ordering::SeqCst);
        ENDED.store(0, Ordering::SeqCst);
    }

    #[cfg(windows)]
    #[test]
    fn install_refuses_a_window_of_another_thread_or_none() {
        assert!(install(0).is_err());
        assert!(install(0x1234).is_err());
        // Not ending while the tests run.
        assert!(!shutting_down());
    }
}
