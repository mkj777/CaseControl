//! The "last good own lighting" seam every driver shares (ADR 0001 #38, #42): what a driver hands back on release,
//! whether that is the device's own lighting, and what it reports for the engine to remember.
//!
//! At open a driver knows three things: what the device shows now (its readback), whether that readback is lighting
//! of the device's own (not another program's direct-mode state), and what the engine tells it through
//! [`DetectContext`](crate::DetectContext): the remembered own lighting (already validated by the driver) and the
//! "unreleased" marker (CaseControl wrote to the device and did not hand it back). [`OwnLighting::at_open`] turns
//! that into one hand-back state and an [`OwnSource`]:
//!
//! | marker | stored | readback | hand-back | source | own lighting to remember | shows own lighting |
//! |---|---|---|---|---|---|---|
//! | set | valid | any | stored | [`Stored`](OwnSource::Stored) | the stored bytes | after the hand-back went out |
//! | set | none | any | readback | [`LeftByUs`](OwnSource::LeftByUs) | none | never (the marker stays) |
//! | clear | any | own | readback | [`Readback`](OwnSource::Readback) | the readback | yes |
//! | clear | any | another program's | readback | [`Foreign`](OwnSource::Foreign) | none | yes |
//!
//! After a write to the device's memory (a save or a restore) the written state is what the device shows on its own
//! from then on ([`OwnSource::Saved`]). Drivers keep the device-specific parts: how to read, validate and encode the
//! state, and when to send the hand-back.

/// Where the state a driver hands back came from (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OwnSource {
    /// Read from the device at open (or when control was taken again after a hand-back): its own lighting.
    Readback,
    /// The remembered own lighting of a device opened with the "unreleased" marker.
    Stored,
    /// The readback of a device opened with the "unreleased" marker and no usable remembered own lighting: a state
    /// CaseControl left on it. Handed back as is, never remembered, and the marker stays.
    LeftByUs,
    /// The readback of a device another program drives (e.g. per-LED mode): handed back as is, never remembered.
    Foreign,
    /// What CaseControl wrote to the device's own memory in this session (a save or a restore).
    Saved,
}

/// The hand-back state of one device and where it came from (see the module docs). `T` is the driver's own
/// representation (a configuration packet, per-zone effect parameters, control params).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnLighting<T> {
    handback: T,
    source: OwnSource,
    shown: bool,
}

impl<T> OwnLighting<T> {
    /// The hand-back state for a device just opened (see the module docs). `stored` must already be validated by
    /// the driver; it is used only when `unreleased`.
    pub fn at_open(readback: T, readback_is_own: bool, stored: Option<T>, unreleased: bool) -> Self {
        let (handback, source) = match (unreleased, stored) {
            (true, Some(stored)) => (stored, OwnSource::Stored),
            (true, None) => (readback, OwnSource::LeftByUs),
            (false, _) if readback_is_own => (readback, OwnSource::Readback),
            (false, _) => (readback, OwnSource::Foreign),
        };
        let shown = matches!(source, OwnSource::Readback | OwnSource::Foreign);
        Self {
            handback,
            source,
            shown,
        }
    }

    /// What a release sends back.
    pub fn handback(&self) -> &T {
        &self.handback
    }

    /// Where [`handback`](Self::handback) came from.
    pub fn source(&self) -> OwnSource {
        self.source
    }

    /// The hand-back state when it is the device's own lighting ([`Readback`](OwnSource::Readback),
    /// [`Stored`](OwnSource::Stored), [`Saved`](OwnSource::Saved)), for the engine to remember; `None` for a state
    /// CaseControl or another program left.
    pub fn own(&self) -> Option<&T> {
        matches!(
            self.source,
            OwnSource::Readback | OwnSource::Stored | OwnSource::Saved
        )
        .then_some(&self.handback)
    }

    /// The answer to [`Controller::shows_own_lighting`](crate::Controller::shows_own_lighting): the device shows (or,
    /// after the hand-back that just succeeded, will show) lighting that is not a state CaseControl left.
    pub fn shows_own_lighting(&self) -> bool {
        self.shown && self.source != OwnSource::LeftByUs
    }

    /// The device does not show the remembered own lighting yet although it is known: a release must send it even if
    /// nothing else was sent since open.
    pub fn handback_pending(&self) -> bool {
        !self.shown && self.own().is_some()
    }

    /// A hand-back (release, or the restore at open) succeeded.
    pub fn handed_back(&mut self) {
        self.shown = true;
    }

    /// `state` was written to the device's own memory and is what the device shows on its own from now on.
    pub fn saved(&mut self, state: T) {
        self.handback = state;
        self.source = OwnSource::Saved;
        self.shown = true;
    }

    /// Control is taken again after a hand-back and the device was read again: the readback is the device's own
    /// lighting, except while the hand-back state is one CaseControl left ([`LeftByUs`](OwnSource::LeftByUs) stays).
    pub fn retaken(&mut self, readback: T) {
        self.handback = readback;
        if self.source != OwnSource::LeftByUs {
            self.source = OwnSource::Readback;
            self.shown = true;
        }
    }

    /// Changes the hand-back state in place (e.g. a header size), keeping its source.
    pub fn handback_mut(&mut self) -> &mut T {
        &mut self.handback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmarked_own_readback() {
        let o = OwnLighting::at_open(1, true, Some(2), false);
        assert_eq!((*o.handback(), o.source()), (1, OwnSource::Readback));
        assert_eq!(o.own(), Some(&1));
        assert!(o.shows_own_lighting());
        assert!(!o.handback_pending());
    }

    #[test]
    fn unmarked_foreign_readback_is_not_remembered() {
        let o = OwnLighting::at_open(1, false, Some(2), false);
        assert_eq!((*o.handback(), o.source()), (1, OwnSource::Foreign));
        assert_eq!(o.own(), None);
        assert!(o.shows_own_lighting());
    }

    #[test]
    fn marked_with_stored_is_pending_until_handed_back() {
        let mut o = OwnLighting::at_open(1, true, Some(2), true);
        assert_eq!((*o.handback(), o.source()), (2, OwnSource::Stored));
        assert_eq!(o.own(), Some(&2));
        assert!(!o.shows_own_lighting());
        assert!(o.handback_pending());
        o.handed_back();
        assert!(o.shows_own_lighting());
        assert!(!o.handback_pending());
    }

    #[test]
    fn marked_without_stored_never_shows_own() {
        let mut o = OwnLighting::at_open(1, true, None, true);
        assert_eq!((*o.handback(), o.source()), (1, OwnSource::LeftByUs));
        assert_eq!(o.own(), None);
        assert!(!o.handback_pending());
        o.handed_back();
        assert!(!o.shows_own_lighting());
        o.retaken(5);
        assert_eq!((*o.handback(), o.source()), (5, OwnSource::LeftByUs));
        assert!(!o.shows_own_lighting());
        o.saved(7);
        assert_eq!((o.own(), o.source()), (Some(&7), OwnSource::Saved));
        assert!(o.shows_own_lighting());
    }

    #[test]
    fn retaken_after_stored_is_readback() {
        let mut o = OwnLighting::at_open(1, true, Some(2), true);
        o.handed_back();
        o.retaken(3);
        assert_eq!((o.own(), o.source()), (Some(&3), OwnSource::Readback));
    }
}
