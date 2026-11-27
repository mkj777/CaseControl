//! The cross-session device lock and the yield signal (ADR 0001 #42).
//!
//! The start-up task's instance runs in session 0, the signed-in instance in the user's session. The app's
//! single-instance guard is per session, so it does not keep them apart. This lock is a named mutex in the `Global\`
//! namespace, one per user SID ([`device_lock_name`]), created with an explicit DACL that grants access only to that
//! user and SYSTEM ([`device_lock_sddl`]). Whoever holds it may drive devices.
//!
//! If another account created a mutex of that name first (squatting), opening it fails with "access denied", or it
//! opens but its owner or DACL is not the user's own ([`check_object_security`]: owner the user, SYSTEM or
//! Administrators, access granted to nobody but the user and SYSTEM). Either way [`LockStatus::HeldBySomeoneElse`]
//! is returned at once, without waiting; the caller decides what to do (the lock never grants access to anything,
//! it only coordinates CaseControl's own instances).
//!
//! The **yield signal** ([`YieldSignal`], a manual-reset event `Global\CaseControl-Yield-<SID>` with the same naming
//! and DACL rules) lets the signed-in instance ask the start-up instance to finish now: the signed-in instance opens
//! it and calls [`YieldSignal::request`] (or [`request_yield`]) and keeps the value alive while it waits for the lock;
//! the start-up instance opens it at start and checks [`YieldSignal::yield_requested`] between steps. The event lives
//! as long as one of them holds a handle, so a request made before the start-up instance opens it is still seen.
//!
//! A Windows mutex belongs to the thread that acquired it. [`DeviceLock`] is therefore not `Send`: drop it on the
//! thread that acquired it (that releases the mutex), or call [`DeviceLock::hold_until_exit`] to keep it for the
//! rest of the process (Windows releases it when the thread or process ends; a later waiter sees it as acquired).

use std::marker::PhantomData;
use std::time::Duration;

use crate::Error;
use crate::acl::{ADMINISTRATORS_SID, AceKind, INHERIT_ONLY_ACE, ObjectSecurity, SYSTEM_SID};
use crate::identity::is_user_sid;

/// The mutex name for the user with `sid`.
pub fn device_lock_name(sid: &str) -> String {
    format!("Global\\CaseControl-Devices-{sid}")
}

/// The yield event's name for the user with `sid`.
pub fn yield_signal_name(sid: &str) -> String {
    format!("Global\\CaseControl-Yield-{sid}")
}

/// The security descriptor of the mutex and of the yield event: protected DACL, generic all for the user and for
/// SYSTEM, nobody else.
pub fn device_lock_sddl(sid: &str) -> String {
    format!("D:P(A;;GA;;;{sid})(A;;GA;;;SY)")
}

/// Pure check of an existing lock or yield object (one this call did not create): the owner is the user `sid`,
/// SYSTEM or Administrators (the default owner of objects an elevated process of the user creates; an administrator
/// is trusted anyway), there is a DACL, and every entry that grants anything grants it to the user or SYSTEM only.
/// `Err` names the first problem.
pub fn check_object_security(security: &ObjectSecurity, sid: &str) -> Result<(), String> {
    let ours = |s: &str| s.eq_ignore_ascii_case(sid) || s.eq_ignore_ascii_case(SYSTEM_SID);
    match &security.owner {
        Some(owner) if ours(owner) || owner.eq_ignore_ascii_case(ADMINISTRATORS_SID) => {}
        Some(owner) => return Err(format!("it is owned by {owner}")),
        None => return Err("its owner cannot be read".to_owned()),
    }
    let Some(dacl) = &security.dacl else {
        return Err("it has no access list (everyone has full access)".to_owned());
    };
    for ace in dacl {
        if ace.flags & INHERIT_ONLY_ACE != 0 || ace.mask == 0 {
            continue;
        }
        match (ace.kind, &ace.sid) {
            (AceKind::Allow, Some(s)) if ours(s) => {}
            (AceKind::Allow, Some(s)) => return Err(format!("it grants access to {s}")),
            (AceKind::Allow, None) | (AceKind::OtherAllow, _) => {
                return Err("an access entry of an unsupported kind grants access".to_owned());
            }
            (AceKind::Deny | AceKind::Other, _) => {}
        }
    }
    Ok(())
}

/// Result of an acquire attempt.
#[derive(Debug)]
pub enum LockStatus {
    /// This thread holds the lock now.
    Acquired(DeviceLock),
    /// Another CaseControl instance of this user holds it (it was not released within the timeout).
    Busy,
    /// A mutex of this name exists that is not this user's: it cannot be opened, or its owner or DACL is foreign.
    /// Returned without waiting.
    HeldBySomeoneElse,
}

/// The held lock; released on drop (on the acquiring thread).
#[derive(Debug)]
pub struct DeviceLock {
    #[cfg(windows)]
    _handle: crate::sys::MutexHandle,
    /// Not `Send` / `Sync`: mutex ownership belongs to the acquiring thread.
    _thread_bound: PhantomData<*const ()>,
}

impl DeviceLock {
    /// Takes the lock for the user with `sid` if it is free right now.
    pub fn try_acquire(sid: &str) -> Result<LockStatus, Error> {
        Self::acquire_with_timeout(sid, Duration::ZERO)
    }

    /// Takes the lock for the user with `sid`, waiting up to `timeout` for another instance to release it.
    pub fn acquire_with_timeout(sid: &str, timeout: Duration) -> Result<LockStatus, Error> {
        if !is_user_sid(sid) {
            return Err(Error::Invalid(format!("{sid:?} is not a user SID")));
        }
        Self::acquire_named(&device_lock_name(sid), &device_lock_sddl(sid), sid, timeout)
    }

    /// Opens or creates mutex `name` (security descriptor `sddl` when created), checks an existing one against
    /// `sid` and waits up to `timeout` for it.
    fn acquire_named(name: &str, sddl: &str, sid: &str, timeout: Duration) -> Result<LockStatus, Error> {
        #[cfg(windows)]
        {
            use crate::sys::{NamedOpen, WaitOutcome};
            let handle = match crate::sys::create_mutex(name, sddl)? {
                NamedOpen::Handle { handle, existed } => {
                    if existed {
                        let verdict = crate::sys::mutex_security(&handle)
                            .map_err(|e| format!("its security cannot be read: {e}"))
                            .and_then(|s| check_object_security(&s, sid));
                        if let Err(why) = verdict {
                            tracing::warn!(%name, %why, "device lock exists but is not this user's");
                            return Ok(LockStatus::HeldBySomeoneElse);
                        }
                    }
                    handle
                }
                NamedOpen::AccessDenied => {
                    tracing::warn!(%name, "device lock exists but belongs to another account");
                    return Ok(LockStatus::HeldBySomeoneElse);
                }
            };
            match crate::sys::wait_mutex(&handle, timeout)? {
                WaitOutcome::Acquired => {}
                WaitOutcome::Abandoned => {
                    tracing::info!("device lock was abandoned by an instance that ended; taking it over");
                }
                WaitOutcome::Timeout => return Ok(LockStatus::Busy),
            }
            Ok(LockStatus::Acquired(DeviceLock {
                _handle: handle,
                _thread_bound: PhantomData,
            }))
        }
        #[cfg(not(windows))]
        {
            let _ = (name, sddl, sid, timeout);
            Err(Error::Unsupported)
        }
    }

    /// Keeps the lock until the process (strictly: the acquiring thread) ends, without releasing it on drop.
    pub fn hold_until_exit(self) {
        std::mem::forget(self);
    }
}

/// Result of opening the yield signal.
#[derive(Debug)]
pub enum SignalStatus {
    /// Opened (or created).
    Opened(YieldSignal),
    /// An object of this name exists that is not this user's; nothing was signalled.
    HeldBySomeoneElse,
}

/// The per-user yield signal (see the module docs). `Send` and `Sync`; the event closes on drop.
#[derive(Debug)]
pub struct YieldSignal {
    #[cfg(windows)]
    handle: crate::sys::EventHandle,
}

impl YieldSignal {
    /// Opens (or creates, not set) the yield signal of the user with `sid`.
    pub fn open(sid: &str) -> Result<SignalStatus, Error> {
        if !is_user_sid(sid) {
            return Err(Error::Invalid(format!("{sid:?} is not a user SID")));
        }
        Self::open_named(&yield_signal_name(sid), &device_lock_sddl(sid), sid)
    }

    fn open_named(name: &str, sddl: &str, sid: &str) -> Result<SignalStatus, Error> {
        #[cfg(windows)]
        {
            use crate::sys::NamedOpen;
            match crate::sys::create_event(name, sddl)? {
                NamedOpen::Handle { handle, existed } => {
                    if existed {
                        let verdict = crate::sys::event_security(&handle)
                            .map_err(|e| format!("its security cannot be read: {e}"))
                            .and_then(|s| check_object_security(&s, sid));
                        if let Err(why) = verdict {
                            tracing::warn!(%name, %why, "yield signal exists but is not this user's");
                            return Ok(SignalStatus::HeldBySomeoneElse);
                        }
                    }
                    Ok(SignalStatus::Opened(YieldSignal { handle }))
                }
                NamedOpen::AccessDenied => {
                    tracing::warn!(%name, "yield signal exists but belongs to another account");
                    Ok(SignalStatus::HeldBySomeoneElse)
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (name, sddl, sid);
            Err(Error::Unsupported)
        }
    }

    /// Asks the start-up instance to finish now. The request stays set while any handle to the signal is open; keep
    /// this value alive until the lock was acquired.
    pub fn request(&self) -> Result<(), Error> {
        #[cfg(windows)]
        {
            crate::sys::set_event(&self.handle)
        }
        #[cfg(not(windows))]
        {
            Err(Error::Unsupported)
        }
    }

    /// Whether the signed-in instance asked to finish (does not wait).
    pub fn yield_requested(&self) -> bool {
        self.wait(Duration::ZERO)
    }

    /// Waits up to `timeout` for a request; whether one was made. A failing wait counts as no request.
    pub fn wait(&self, timeout: Duration) -> bool {
        #[cfg(windows)]
        {
            crate::sys::wait_event(&self.handle, timeout).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "waiting for the yield signal failed");
                false
            })
        }
        #[cfg(not(windows))]
        {
            let _ = timeout;
            false
        }
    }
}

/// Opens the yield signal of the user with `sid` and sets it (see [`YieldSignal::request`]). Keep the returned
/// signal alive while waiting for the lock.
pub fn request_yield(sid: &str) -> Result<SignalStatus, Error> {
    let status = YieldSignal::open(sid)?;
    if let SignalStatus::Opened(signal) = &status {
        signal.request()?;
        tracing::info!("asked the start-up instance to finish");
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acl::Ace;

    const SID: &str = "S-1-5-21-1-2-3-1001";
    const OTHER: &str = "S-1-5-21-1-2-3-1002";

    #[test]
    fn name_and_sddl() {
        assert_eq!(
            device_lock_name(SID),
            r"Global\CaseControl-Devices-S-1-5-21-1-2-3-1001"
        );
        assert_eq!(
            yield_signal_name(SID),
            r"Global\CaseControl-Yield-S-1-5-21-1-2-3-1001"
        );
        assert_eq!(
            device_lock_sddl(SID),
            "D:P(A;;GA;;;S-1-5-21-1-2-3-1001)(A;;GA;;;SY)"
        );
    }

    #[test]
    fn non_user_sids_are_refused() {
        assert!(DeviceLock::try_acquire("S-1-5-18").is_err());
        assert!(DeviceLock::try_acquire("x)(A;;GA;;;WD").is_err());
        assert!(YieldSignal::open("S-1-5-18").is_err());
        assert!(request_yield("x)(A;;GA;;;WD").is_err());
    }

    fn allow(sid: &str) -> Ace {
        Ace {
            kind: AceKind::Allow,
            sid: Some(sid.into()),
            mask: 0x001F_0001,
            flags: 0,
        }
    }

    fn security(owner: Option<&str>, dacl: Option<Vec<Ace>>) -> ObjectSecurity {
        ObjectSecurity {
            owner: owner.map(Into::into),
            dacl,
        }
    }

    #[test]
    fn existing_object_security_is_checked() {
        // As CaseControl creates it.
        let own = security(Some(SID), Some(vec![allow(SID), allow(SYSTEM_SID)]));
        assert_eq!(check_object_security(&own, SID), Ok(()));
        assert_eq!(
            check_object_security(&security(Some(SYSTEM_SID), Some(vec![allow(SID)])), SID),
            Ok(())
        );
        // A squatter that grants the user access but owns the object.
        assert!(check_object_security(&security(Some(OTHER), Some(vec![allow(SID)])), SID).is_err());
        // Owned by Administrators (an elevated instance of the user) with our DACL: accepted.
        assert_eq!(
            check_object_security(&security(Some(ADMINISTRATORS_SID), Some(vec![allow(SID)])), SID),
            Ok(())
        );
        // Our owner, but someone else may open it too.
        for extra in [OTHER, "S-1-1-0", "S-1-5-32-544"] {
            let s = security(Some(SID), Some(vec![allow(SID), allow(extra)]));
            assert!(check_object_security(&s, SID).is_err(), "{extra}");
        }
        // NULL DACL, unreadable owner, entry of an unknown kind.
        assert!(check_object_security(&security(Some(SID), None), SID).is_err());
        assert!(check_object_security(&security(None, Some(vec![allow(SID)])), SID).is_err());
        let other_kind = Ace {
            kind: AceKind::OtherAllow,
            sid: None,
            mask: 1,
            flags: 0,
        };
        assert!(check_object_security(&security(Some(SID), Some(vec![other_kind])), SID).is_err());
        // Deny entries and inherit-only entries do not grant anything.
        let deny = Ace {
            kind: AceKind::Deny,
            ..allow(OTHER)
        };
        let inherit_only = Ace {
            flags: INHERIT_ONLY_ACE,
            ..allow(OTHER)
        };
        let s = security(Some(SID), Some(vec![allow(SID), deny, inherit_only]));
        assert_eq!(check_object_security(&s, SID), Ok(()));
    }

    /// Creates a real mutex under a test-only name (so a running CaseControl is never touched), takes it, and checks
    /// that a second thread sees it busy until it is released. Mutexes vanish with their last handle.
    #[cfg(windows)]
    #[test]
    fn real_lock_is_exclusive_across_threads() {
        let sid = crate::current_user_sid().expect("sid");
        let name = format!(r"Global\CaseControl-Test-{}-{}", std::process::id(), line!());
        let sddl = device_lock_sddl(&sid);
        let attempt = |timeout| DeviceLock::acquire_named(&name, &sddl, &sid, timeout);
        let LockStatus::Acquired(lock) = attempt(Duration::ZERO).expect("acquire") else {
            panic!("a fresh lock must be free");
        };
        std::thread::scope(|s| {
            let busy = s
                .spawn(|| matches!(attempt(Duration::ZERO), Ok(LockStatus::Busy)))
                .join()
                .expect("thread");
            assert!(busy);
        });
        drop(lock);
        std::thread::scope(|s| {
            let free = s.spawn(|| matches!(attempt(Duration::from_millis(500)), Ok(LockStatus::Acquired(_))));
            assert!(free.join().expect("thread"));
        });
    }

    /// A mutex of the name that another account created (here: a DACL for a SID that is not ours) cannot be opened
    /// and is reported as held by someone else.
    #[cfg(windows)]
    #[test]
    fn real_lock_created_by_another_account_is_reported() {
        let sid = crate::current_user_sid().expect("sid");
        let name = format!(r"Global\CaseControl-Test-{}-{}", std::process::id(), line!());
        let foreign = device_lock_sddl("S-1-5-21-1-2-3-4242");
        let first = DeviceLock::acquire_named(&name, &foreign, &sid, Duration::ZERO).expect("create");
        assert!(matches!(first, LockStatus::Acquired(_)));
        let second = DeviceLock::acquire_named(&name, &foreign, &sid, Duration::ZERO).expect("open");
        assert!(matches!(second, LockStatus::HeldBySomeoneElse), "{second:?}");
        drop(first);
    }

    /// A mutex that opens (the DACL lets this user in) but also lets everyone in is not trusted: held by someone
    /// else, returned at once even with a long timeout.
    #[cfg(windows)]
    #[test]
    fn real_lock_with_a_foreign_dacl_is_reported_without_waiting() {
        let sid = crate::current_user_sid().expect("sid");
        let name = format!(r"Global\CaseControl-Test-{}-{}", std::process::id(), line!());
        let open_to_all = format!("D:P(A;;GA;;;{sid})(A;;GA;;;WD)");
        let first = DeviceLock::acquire_named(&name, &open_to_all, &sid, Duration::ZERO).expect("create");
        assert!(matches!(first, LockStatus::Acquired(_)));
        let started = std::time::Instant::now();
        let second = std::thread::scope(|s| {
            s.spawn(|| {
                let status = DeviceLock::acquire_named(&name, &open_to_all, &sid, Duration::from_secs(10));
                format!("{status:?}")
            })
            .join()
            .expect("thread")
        });
        assert_eq!(second, "Ok(HeldBySomeoneElse)");
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(first);
    }

    /// The yield signal under a test-only name: not set when opened, seen by a second handle once requested.
    #[cfg(windows)]
    #[test]
    fn real_yield_signal_round_trip() {
        let sid = crate::current_user_sid().expect("sid");
        let name = format!(r"Global\CaseControl-Test-{}-{}", std::process::id(), line!());
        let sddl = device_lock_sddl(&sid);
        let SignalStatus::Opened(watch) = YieldSignal::open_named(&name, &sddl, &sid).expect("open") else {
            panic!("own signal");
        };
        assert!(!watch.yield_requested());
        let SignalStatus::Opened(asker) = YieldSignal::open_named(&name, &sddl, &sid).expect("open again")
        else {
            panic!("own signal, existing");
        };
        asker.request().expect("set");
        let seen = std::thread::scope(|s| {
            s.spawn(|| watch.wait(Duration::from_secs(2)))
                .join()
                .expect("thread")
        });
        assert!(seen);
        assert!(watch.yield_requested(), "manual reset: stays set");

        // A signal of that name owned by a foreign DACL is refused.
        let name = format!(r"Global\CaseControl-Test-{}-{}", std::process::id(), line!());
        let foreign = format!("D:P(A;;GA;;;{sid})(A;;GA;;;WD)");
        let _first = YieldSignal::open_named(&name, &foreign, &sid).expect("create");
        assert!(matches!(
            YieldSignal::open_named(&name, &foreign, &sid),
            Ok(SignalStatus::HeldBySomeoneElse)
        ));
    }
}
