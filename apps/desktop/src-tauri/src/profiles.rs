//! Profile bookkeeping shared by the commands, the tray and the start: the list the profile menu shows, the names of
//! new and duplicated profiles, which profile is active after a start or a delete, and how an autosave merges what
//! the devices show into the active profile (ADR 0001 #40). Profile names compare case-insensitively after trimming,
//! like `cc_config::ProfileStore` does.

use std::collections::BTreeMap;

use cc_config::{DeviceSetting, MAX_NAME_LEN, Profile};
use cc_core::DeviceId;
use serde::Serialize;

/// The profile menu's list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileList {
    /// Profile names, sorted case-insensitively.
    pub names: Vec<String>,
    /// The active profile, spelled as in `names` when it exists there.
    pub active: Option<String>,
}

/// Whether two profile names denote the same profile.
pub fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

fn is_taken(names: &[String], candidate: &str) -> bool {
    names.iter().any(|n| same_name(n, candidate))
}

/// Builds the list; `active` is only set when `last_profile` names an existing profile.
pub fn profile_list(names: Vec<String>, last_profile: Option<&str>) -> ProfileList {
    let active = last_profile.and_then(|last| names.iter().find(|n| same_name(n, last)).cloned());
    ProfileList { names, active }
}

/// The new `last_profile` after profile `from` was renamed to `to`; `None` when nothing changes.
pub fn last_after_rename(last: Option<&str>, from: &str, to: &str) -> Option<String> {
    last.filter(|l| same_name(l, from)).map(|_| to.trim().to_owned())
}

/// The name of a new profile: "Profile N" with the lowest N >= 1 that no profile in `names` has.
pub fn next_profile_name(names: &[String]) -> String {
    (1u32..)
        .map(|n| format!("Profile {n}"))
        .find(|candidate| !is_taken(names, candidate))
        .expect("an unbounded range always finds a free name")
}

/// The name of a copy of profile `source`: `"<source> copy"`, then `"<source> copy 2"`, `"<source> copy 3"`, ...,
/// the first that no profile in `names` has. When the result would be longer than [`MAX_NAME_LEN`] characters, the
/// end of `source` is cut off so the suffix fits.
pub fn duplicate_name(source: &str, names: &[String]) -> String {
    let source = source.trim();
    let with_suffix = |suffix: &str| {
        let keep = MAX_NAME_LEN.saturating_sub(suffix.chars().count());
        let base: String = source.chars().take(keep).collect();
        format!("{}{suffix}", base.trim_end())
    };
    std::iter::once(" copy".to_owned())
        .chain((2u32..).map(|n| format!(" copy {n}")))
        .map(|suffix| with_suffix(&suffix))
        .find(|candidate| !is_taken(names, candidate))
        .expect("an unbounded range always finds a free name")
}

/// What the start does about profiles (ADR 0001 #40): there is always exactly one active profile afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartPlan {
    /// No profile exists: create this one from what the devices show now and make it active.
    Create(String),
    /// Apply this existing profile; `remember` when `last_profile` has to be set to it (it was empty or named a
    /// profile that does not exist).
    Apply {
        /// The profile, spelled as in the list.
        name: String,
        /// Whether `last_profile` must be updated.
        remember: bool,
    },
}

/// The start plan for the profiles in `names` (sorted as the list) and the saved `last_profile`.
pub fn start_plan(names: &[String], last: Option<&str>) -> StartPlan {
    let Some(first) = names.first() else {
        return StartPlan::Create(next_profile_name(names));
    };
    match last.and_then(|l| names.iter().find(|n| same_name(n, l))) {
        Some(name) => StartPlan::Apply {
            name: name.clone(),
            remember: false,
        },
        None => StartPlan::Apply {
            name: first.clone(),
            remember: true,
        },
    }
}

/// Why a profile cannot be deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteRefused {
    /// No profile has that name.
    NotFound,
    /// It is the only profile; there must always be one (ADR 0001 #40).
    Last,
}

impl std::fmt::Display for DeleteRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotFound => "There is no profile with that name.",
            Self::Last => "The last profile cannot be deleted.",
        })
    }
}

/// Checks deleting profile `deleted` from `names` (sorted as the list) while `active` is the active profile. Returns
/// the profile to activate afterwards: the first remaining one when the active profile is deleted, else `None`.
pub fn plan_delete(
    names: &[String],
    active: Option<&str>,
    deleted: &str,
) -> Result<Option<String>, DeleteRefused> {
    if !is_taken(names, deleted) {
        return Err(DeleteRefused::NotFound);
    }
    let mut remaining = names.iter().filter(|n| !same_name(n, deleted));
    let Some(first) = remaining.next() else {
        return Err(DeleteRefused::Last);
    };
    let deletes_active = active.is_none_or(|a| same_name(a, deleted));
    Ok(deletes_active.then(|| first.clone()))
}

/// Merges what the devices show into the active profile for an autosave (ADR 0001 #40): every device the engine has
/// a setting for gets it, entries of devices that are not connected (or have no setting now) stay, and the device
/// brightness is replaced by the engine's map (which holds every device, connected or not).
pub fn merge_snapshot(
    profile: &mut Profile,
    current: BTreeMap<DeviceId, DeviceSetting>,
    brightness: BTreeMap<DeviceId, u8>,
) {
    profile.devices.extend(current);
    profile.brightness = brightness;
}

#[cfg(test)]
mod tests {
    use cc_core::Rgb;

    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn names_compare_case_insensitively_and_trimmed() {
        assert!(same_name("Evening", " evening "));
        assert!(!same_name("Evening", "Morning"));
    }

    #[test]
    fn list_marks_existing_active_profile() {
        let names = names(&["Day", "Evening"]);
        let l = profile_list(names.clone(), Some("evening"));
        assert_eq!(l.active.as_deref(), Some("Evening"));
        assert_eq!(profile_list(names.clone(), Some("Gone")).active, None);
        assert_eq!(profile_list(names, None).active, None);
    }

    #[test]
    fn rename_follows_last_profile() {
        assert_eq!(
            last_after_rename(Some("Evening"), "evening", " Night "),
            Some("Night".to_owned())
        );
        assert_eq!(last_after_rename(Some("Day"), "Evening", "Night"), None);
        assert_eq!(last_after_rename(None, "Evening", "Night"), None);
    }

    #[test]
    fn next_profile_name_takes_the_lowest_free_number() {
        assert_eq!(next_profile_name(&[]), "Profile 1");
        assert_eq!(next_profile_name(&names(&["Evening"])), "Profile 1");
        assert_eq!(
            next_profile_name(&names(&["Profile 1", "Profile 2"])),
            "Profile 3"
        );
        assert_eq!(
            next_profile_name(&names(&["profile 1", " PROFILE 3 ", "Profile 4"])),
            "Profile 2",
            "case-insensitive and trimmed; gaps are filled"
        );
        assert_eq!(
            next_profile_name(&names(&["Profile 0", "Profile 2"])),
            "Profile 1"
        );
    }

    #[test]
    fn duplicate_name_counts_up_copies() {
        assert_eq!(duplicate_name("Evening", &names(&["Evening"])), "Evening copy");
        assert_eq!(
            duplicate_name("Evening", &names(&["Evening", "evening COPY"])),
            "Evening copy 2"
        );
        assert_eq!(
            duplicate_name(
                " Evening ",
                &names(&["Evening", "Evening copy", "Evening copy 2"])
            ),
            "Evening copy 3"
        );
        // A copy of a copy.
        assert_eq!(
            duplicate_name("Evening copy", &names(&["Evening", "Evening copy"])),
            "Evening copy copy"
        );
    }

    #[test]
    fn duplicate_name_fits_the_name_limit() {
        let long = "x".repeat(MAX_NAME_LEN);
        let copy = duplicate_name(&long, std::slice::from_ref(&long));
        assert_eq!(copy.chars().count(), MAX_NAME_LEN);
        assert!(copy.ends_with(" copy"));
        let copy2 = duplicate_name(&long, &[long.clone(), copy.clone()]);
        assert_eq!(copy2.chars().count(), MAX_NAME_LEN);
        assert!(copy2.ends_with(" copy 2"));
        assert!(cc_config::validate_name(&copy2).is_ok());
    }

    #[test]
    fn start_without_profiles_creates_profile_1() {
        assert_eq!(start_plan(&[], None), StartPlan::Create("Profile 1".into()));
        assert_eq!(
            start_plan(&[], Some("Evening")),
            StartPlan::Create("Profile 1".into())
        );
    }

    #[test]
    fn start_with_dangling_or_empty_last_profile_activates_the_first() {
        let list = names(&["Day", "Evening"]);
        let first = StartPlan::Apply {
            name: "Day".into(),
            remember: true,
        };
        assert_eq!(start_plan(&list, None), first);
        assert_eq!(start_plan(&list, Some("Gone")), first);
        assert_eq!(
            start_plan(&list, Some(" evening")),
            StartPlan::Apply {
                name: "Evening".into(),
                remember: false
            }
        );
    }

    #[test]
    fn last_profile_cannot_be_deleted() {
        assert_eq!(
            plan_delete(&names(&["Only"]), Some("Only"), "only"),
            Err(DeleteRefused::Last)
        );
        assert_eq!(
            plan_delete(&names(&["Only"]), None, "Only"),
            Err(DeleteRefused::Last)
        );
        assert_eq!(
            plan_delete(&names(&["Only"]), Some("Only"), "Other"),
            Err(DeleteRefused::NotFound)
        );
    }

    #[test]
    fn deleting_the_active_profile_activates_the_first_remaining() {
        let list = names(&["Alpha", "Beta", "Gamma"]);
        assert_eq!(plan_delete(&list, Some("beta"), "Beta"), Ok(Some("Alpha".into())));
        assert_eq!(
            plan_delete(&list, Some("Alpha"), "alpha"),
            Ok(Some("Beta".into()))
        );
        assert_eq!(
            plan_delete(&list, Some("Alpha"), "Gamma"),
            Ok(None),
            "another profile stays active"
        );
        assert_eq!(plan_delete(&list, None, "Gamma"), Ok(Some("Alpha".into())));
    }

    #[test]
    fn snapshot_overwrites_connected_devices_and_keeps_the_rest() {
        let connected = DeviceId::hid(0x1462, 0x7D75, None);
        let away = DeviceId::hid(0x31E3, 0x1310, None);
        let direct = |c: Rgb| DeviceSetting::Direct { colors: vec![c] };
        let mut profile = Profile {
            name: "P".into(),
            devices: BTreeMap::from([
                (connected.clone(), direct(Rgb::BLACK)),
                (away.clone(), direct(Rgb::WHITE)),
            ]),
            brightness: BTreeMap::from([(away.clone(), 10)]),
        };
        let red = direct(Rgb::new(255, 0, 0));
        merge_snapshot(
            &mut profile,
            BTreeMap::from([(connected.clone(), red.clone())]),
            BTreeMap::from([(connected.clone(), 40)]),
        );
        assert_eq!(profile.devices[&connected], red);
        assert_eq!(profile.devices[&away], direct(Rgb::WHITE));
        assert_eq!(profile.brightness, BTreeMap::from([(connected, 40)]));
    }

    #[test]
    fn list_serializes_to_frontend_shape() {
        let json = serde_json::to_value(profile_list(vec!["A".into()], Some("a"))).expect("serializes");
        assert_eq!(json, serde_json::json!({ "names": ["A"], "active": "A" }));
    }
}
