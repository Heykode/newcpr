use chrono::{Duration, Utc};
use gateway_admin::model::{
    provider_credentials::{ProviderResetCredit, ProviderResetCredits},
    reset_credits::earliest_credit,
};

fn card(id: &str, hours: i64, kind: &str) -> ProviderResetCredit {
    ProviderResetCredit {
        id: id.into(),
        status: Some("available".into()),
        title: None,
        expires_at: Some(Utc::now() + Duration::hours(hours)),
        reset_type: Some(kind.into()),
    }
}

#[test]
fn reset_credits_earliest_valid_expiry_wins_not_upstream_order() {
    let mut inventory = ProviderResetCredits {
        available_count: 4,
        credits: vec![
            card("later", 48, "codex"),
            card("expired", -1, "codex"),
            card("soon", 1, "codex"),
            card("tomorrow", 24, "codex"),
        ],
    };
    assert_eq!(
        earliest_credit(&inventory, None, Utc::now()).unwrap().id,
        "soon"
    );
    inventory.credits[2].status = Some("redeemed".into());
    assert_eq!(
        earliest_credit(&inventory, None, Utc::now()).unwrap().id,
        "tomorrow"
    );
    inventory.available_count = 0;
    assert!(earliest_credit(&inventory, None, Utc::now()).is_err());
}

#[test]
fn reset_credits_mixed_windows_require_explicit_type() {
    let inventory = ProviderResetCredits {
        available_count: 2,
        credits: vec![card("weekly", 1, "weekly"), card("short", 2, "short")],
    };
    assert!(earliest_credit(&inventory, None, Utc::now()).is_err());
    assert_eq!(
        earliest_credit(&inventory, Some("short"), Utc::now())
            .unwrap()
            .id,
        "short"
    );
    assert!(earliest_credit(&inventory, Some("missing"), Utc::now()).is_err());
}

#[test]
fn reset_credits_missing_expiry_duplicates_and_count_only_are_not_guessed() {
    let mut inventory = ProviderResetCredits {
        available_count: 2,
        credits: vec![card("one", 1, "codex")],
    };
    inventory.credits[0].expires_at = None;
    assert!(earliest_credit(&inventory, None, Utc::now()).is_err());
    inventory.credits = vec![card("one", 1, "codex"), card("one", 2, "codex")];
    assert!(earliest_credit(&inventory, None, Utc::now()).is_err());
    inventory.credits.clear();
    assert!(earliest_credit(&inventory, None, Utc::now()).is_err());
}
