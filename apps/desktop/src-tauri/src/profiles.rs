//! Profile bookkeeping shared by the commands and the tray: the list the Profiles page shows and how
//! `last_profile` follows renames and deletes. Profile names compare case-insensitively after trimming, like
//! `cc_config::ProfileStore` does.

use serde::Serialize;

/// The Profiles page's list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileList {
    /// Profile names, sorted case-insensitively.
    pub names: Vec<String>,
    /// The profile applied last, spelled as in `names` when it exists there.
    pub active: Option<String>,
}

/// Whether two profile names denote the same profile.
pub fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
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

/// Whether `last_profile` must be cleared because profile `deleted` is gone.
pub fn clears_last(last: Option<&str>, deleted: &str) -> bool {
    last.is_some_and(|l| same_name(l, deleted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_compare_case_insensitively_and_trimmed() {
        assert!(same_name("Evening", " evening "));
        assert!(!same_name("Evening", "Morning"));
    }

    #[test]
    fn list_marks_existing_active_profile() {
        let names = vec!["Day".to_owned(), "Evening".to_owned()];
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
    fn delete_clears_matching_last_profile() {
        assert!(clears_last(Some("Evening"), "EVENING"));
        assert!(!clears_last(Some("Day"), "Evening"));
        assert!(!clears_last(None, "Evening"));
    }

    #[test]
    fn list_serializes_to_frontend_shape() {
        let json = serde_json::to_value(profile_list(vec!["A".into()], Some("a"))).expect("serializes");
        assert_eq!(json, serde_json::json!({ "names": ["A"], "active": "A" }));
    }
}
