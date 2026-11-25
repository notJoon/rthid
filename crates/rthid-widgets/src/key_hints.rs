//! Formatting for groups of key descriptions.

/// Joins key and action pairs into one help line.
///
/// Each pair becomes `key: action` and pairs are joined with ` | ` in the
/// given order. Groups let callers combine hints from several components,
/// such as [`Confirm::HINTS`](crate::Confirm::HINTS), with their own. Empty
/// groups add nothing and no groups give an empty string.
///
/// # Examples
///
/// ```
/// use rthid_widgets::key_hints;
///
/// let picker: &[(&str, &str)] = &[("j/k", "move"), ("Enter", "open")];
/// let app: &[(&str, &str)] = &[("q", "quit")];
/// assert_eq!(key_hints(&[picker, &[], app]), "j/k: move | Enter: open | q: quit");
/// assert_eq!(key_hints(&[]), "");
/// ```
pub fn key_hints(groups: &[&[(&str, &str)]]) -> String {
    groups
        .iter()
        .flat_map(|group| group.iter())
        .map(|(key, action)| format!("{key}: {action}"))
        .collect::<Vec<_>>()
        .join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_groups_in_order() {
        assert_eq!(
            key_hints(&[&[("a", "add"), ("d", "delete")], &[], &[("Esc", "cancel")]]),
            "a: add | d: delete | Esc: cancel"
        );
        assert_eq!(key_hints(&[]), "");
    }
}
