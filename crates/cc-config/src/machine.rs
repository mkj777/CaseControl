//! This machine's id, which binds the machine state to the machine it was written on (ADR 0001 #42).

/// The value of `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid` (read only), set by Windows setup and unique per
/// installation; `None` when it cannot be read (or on other platforms). Never written.
pub fn machine_id() -> Option<String> {
    imp::read()
}

#[cfg(windows)]
mod imp {
    pub(super) fn read() -> Option<String> {
        let key = windows_registry::LOCAL_MACHINE
            .open(r"SOFTWARE\Microsoft\Cryptography")
            .map_err(|e| tracing::warn!(error = %e, "cannot open the Cryptography key to read MachineGuid"))
            .ok()?;
        match key.get_string("MachineGuid") {
            Ok(id) if !id.trim().is_empty() => Some(id.trim().to_owned()),
            Ok(_) => {
                tracing::warn!("MachineGuid is empty");
                None
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot read MachineGuid");
                None
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub(super) fn read() -> Option<String> {
        None
    }
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn reads_the_machine_guid() {
        // Read-only registry access; MachineGuid exists on every Windows installation.
        let id = super::machine_id().expect("MachineGuid readable");
        assert!(!id.is_empty());
        assert_eq!(super::machine_id(), Some(id), "stable");
    }
}
