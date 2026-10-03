use std::env;

pub fn settle(value: Option<String>, current: &str, legacy: &str) -> Option<String> {
    settle_with(value, current, legacy, |name| env::var(name).ok())
}

fn settle_with(
    value: Option<String>,
    current: &str,
    legacy: &str,
    lookup: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    value.or_else(|| {
        let found = lookup(legacy).filter(|found| !found.is_empty())?;
        eprintln!(
            "warning: {legacy} is deprecated and will stop being read; set {current} instead"
        );
        Some(found)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only_legacy(name: &str) -> Option<String> {
        (name == "ROXYCLOUD_TOKEN").then(|| "old-token".to_owned())
    }

    #[test]
    fn the_current_name_wins_over_the_legacy_one() {
        let settled = settle_with(
            Some("new-token".to_owned()),
            "STASHDEN_TOKEN",
            "ROXYCLOUD_TOKEN",
            only_legacy,
        );
        assert_eq!(settled.as_deref(), Some("new-token"));
    }

    #[test]
    fn the_legacy_name_is_still_read_when_the_current_one_is_absent() {
        let settled = settle_with(None, "STASHDEN_TOKEN", "ROXYCLOUD_TOKEN", only_legacy);
        assert_eq!(settled.as_deref(), Some("old-token"));
    }

    #[test]
    fn an_empty_legacy_value_counts_as_absent() {
        let settled = settle_with(None, "STASHDEN_URL", "ROXYCLOUD_URL", |_| {
            Some(String::new())
        });
        assert_eq!(settled, None);
    }

    #[test]
    fn nothing_set_settles_on_nothing() {
        let settled = settle_with(None, "STASHDEN_URL", "ROXYCLOUD_URL", |_| None);
        assert_eq!(settled, None);
    }
}
