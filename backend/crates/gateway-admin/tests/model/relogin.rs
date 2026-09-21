use gateway_admin::model::relogin::{ReloginSettings, parse_relogin_import, validate_ids};

#[test]
fn relogin_parser_preserves_password_and_normalizes_email_and_totp() {
    let rows = parse_relogin_import(
        "\u{feff} Test@Example.invalid ---- p%a----ss ----jbsw y3dp-ehpk3pxp\r\n\n",
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].email, "test@example.invalid");
    assert_eq!(rows[0].password, " p%a----ss ");
    assert_eq!(rows[0].mfa_secret, "JBSWY3DPEHPK3PXP");
}

#[test]
fn relogin_parser_rejects_invalid_rows_without_exposing_secrets() {
    for text in [
        "",
        "test@example.invalid----private-password",
        "test@example.invalid--------JBSWY3DPEHPK3PXP",
        "test@example.invalid----private-password----JBSWY3DPEHPK3PXP0",
        "test@example.invalid----private-password----JBSWY3DPEHPK3PX",
        "bad address@example.invalid----private-password----JBSWY3DPEHPK3PXP",
    ] {
        let error = parse_relogin_import(text).err().expect("invalid input");
        assert!(!error.message().contains("private-password"));
        assert!(!error.message().contains("JBSWY"));
    }
    let duplicate =
        "a@example.invalid----p----JBSWY3DPEHPK3PXP\nA@example.invalid----p----JBSWY3DPEHPK3PXP";
    assert!(parse_relogin_import(duplicate).is_err());
    let too_many = (0..501)
        .map(|index| format!("a{index}@example.invalid----p----JBSWY3DPEHPK3PXP"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(parse_relogin_import(&too_many).is_err());
    assert!(parse_relogin_import(&"x".repeat(512 * 1024 + 1)).is_err());
}

#[test]
fn relogin_parser_accepts_totp_for_outlook_addresses() {
    let rows =
        parse_relogin_import("test@outlook.com----test-only-password----JBSWY3DPEHPK3PXP").unwrap();
    assert_eq!(rows[0].email, "test@outlook.com");
    assert_eq!(rows[0].password, "test-only-password");
    assert_eq!(rows[0].mfa_secret, "JBSWY3DPEHPK3PXP");
}

#[test]
fn relogin_parser_rejects_all_mailbox_formats() {
    for text in [
        "test@outlook.com----p----123e4567-e89b-12d3-a456-426614174000",
        "test@outlook.com----p----123e4567-e89b-12d3-a456-426614174000----!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!",
        "test@outlook.com----p----123e4567-e89b-12d3-a456-426614174000----short",
        "test@outlook.com----p----123e4567-e89b-12d3-a456-426614174000----JBSWY3DPEHPK3PXP",
        "test@outlook.com|synthetic-mailbox-token|123e4567-e89b-12d3-a456-426614174000",
    ] {
        assert!(parse_relogin_import(text).is_err());
    }
}

#[test]
fn relogin_settings_and_batch_limits_are_bounded() {
    assert_eq!(ReloginSettings::default().concurrency, 1);
    assert!(!ReloginSettings::default().paused);
    assert_eq!(ReloginSettings::default().max_retries, 2);
    assert_eq!(ReloginSettings::default().retry_interval_minutes, 5);
    for (max_retries, retry_interval_minutes, valid) in [
        (0, 1, true),
        (10, 1440, true),
        (11, 5, false),
        (2, 0, false),
        (2, 1441, false),
        (u32::MAX, u32::MAX, false),
    ] {
        assert_eq!(
            ReloginSettings {
                max_retries,
                retry_interval_minutes,
                ..ReloginSettings::default()
            }
            .validate()
            .is_ok(),
            valid
        );
    }
    for concurrency in [0, 9, usize::MAX] {
        assert!(
            ReloginSettings {
                concurrency,
                paused: false,
                ..ReloginSettings::default()
            }
            .validate()
            .is_err()
        );
    }
    assert!(validate_ids(&[]).is_err());
    assert!(validate_ids(&["a".into(), "a".into()]).is_err());
    assert!(validate_ids(&["a".into()]).is_ok());
}
