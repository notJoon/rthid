//! Formatting for groups of key descriptions.

/// Preserves group and pair order with a separator between hints.
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
