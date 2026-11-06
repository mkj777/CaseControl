//! Device family ids.
//!
//! Detectors are toggled per family (`cc-config` `detectors` table), drivers report which family they belong to,
//! and the conflict catalog names the families a conflicting tool affects. All of them use these constants.

/// MSI Mystic Light motherboards (board LEDs and ARGB headers).
pub const MSI_MYSTIC_LIGHT: &str = "msi_mystic_light";
/// Wooting keyboards.
pub const WOOTING: &str = "wooting";
/// Logitech HID++ devices (speakers, Lightspeed receivers).
pub const LOGITECH: &str = "logitech";
/// NVIDIA GPUs controlled through NvAPI Illumination.
pub const NVIDIA_ILLUMINATION: &str = "nvidia_illumination";

/// Every family id known to this version.
pub const ALL: &[&str] = &[MSI_MYSTIC_LIGHT, WOOTING, LOGITECH, NVIDIA_ILLUMINATION];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_snake_case() {
        let set: HashSet<_> = ALL.iter().collect();
        assert_eq!(set.len(), ALL.len());
        for id in ALL {
            assert!(id.bytes().all(|c| c.is_ascii_lowercase() || c == b'_'), "{id}");
        }
    }
}
