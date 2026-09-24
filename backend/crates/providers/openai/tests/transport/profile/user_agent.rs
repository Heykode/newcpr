use gateway_core::provider_ports::ProviderUserAgentOverride;
use provider_openai::transport::profile::{
    CodexWireProfile, CodexWireProfileError, CodexWireProfileOverride, CodexWireProfileState, qx,
};

use super::{CodexBundledReleaseProfile, Utc, wire_profile};

const CUSTOM: &str =
    "Codex Desktop/0.153.4 (Mac OS 15.7.1; arm64) unknown (Codex Desktop; 26.901.51231)";
const EXEC: &str =
    "codex_exec/0.156.1 (Ubuntu 24.4.0; x86_64) xterm-256color (codex_exec; 0.156.1)";

#[test]
fn request_profile_snapshot_keeps_custom_ua_and_auxiliary_desktop_across_updates() {
    for custom in [
        None,
        Some(CUSTOM),
        Some("codex_cli_rs/0.153.4 (Linux 6.8; x86_64) xterm"),
        Some(EXEC),
    ] {
        let state = CodexWireProfileState::new(wire_profile());
        if let Some(user_agent) = custom {
            state
                .apply_user_agent_override(&ProviderUserAgentOverride::Custom {
                    user_agent: user_agent.to_owned(),
                })
                .unwrap();
        }
        let original = state.snapshot();
        let desktop = state.desktop_snapshot();
        let frozen = state.request_snapshot().unwrap();
        state.update_bundled_release(&CodexBundledReleaseProfile {
            codex_version: "0.154.0".to_owned(),
            desktop_version: "26.912.12345".to_owned(),
            desktop_build: "9012".to_owned(),
            verified_at: Utc::now(),
        });
        state
            .apply_user_agent_override(&ProviderUserAgentOverride::Default)
            .unwrap();
        for _ in 0..3 {
            let retry = CodexWireProfileState::from_request_snapshot(&frozen).unwrap();
            assert_eq!(retry.snapshot(), original);
            assert_eq!(retry.desktop_snapshot(), desktop);
        }
        let next_request =
            CodexWireProfileState::from_request_snapshot(&state.request_snapshot().unwrap())
                .unwrap();
        assert_eq!(next_request.snapshot().codex_version, "0.154.0");
        assert_ne!(next_request.snapshot(), original);
        let keys: Vec<_> = frozen
            .expose_to_provider()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["selection"]);
    }
}

#[test]
fn custom_user_agent_preserves_coherent_core_and_desktop_surfaces() {
    let profile = CodexWireProfile::parse_user_agent(CUSTOM).expect("supported UA");
    assert_eq!(profile.user_agent(), CUSTOM);
    assert_eq!(profile.codex_version, "0.153.4");
    assert_eq!(profile.originator, "Codex Desktop");
    assert_eq!(
        profile.desktop_user_agent(),
        "Codex Desktop/26.901.51231 (Mac OS; arm64)"
    );
    assert_eq!(profile.desktop_build, "custom");
}

#[test]
fn user_agent_parser_rejects_unsafe_or_incoherent_values() {
    for value in [
        "".to_owned(),
        format!("{CUSTOM}\r\nAuthorization: injected"),
        CUSTOM.replace("arm64", "windows"),
        CUSTOM.replace("15.7.1", "15.7.1; injected"),
        CUSTOM.replace("0.153.4", "latest"),
        CUSTOM.replace("26.901.51231", "latest"),
        CUSTOM.replacen("Codex Desktop", "Codex CLI", 1),
        CUSTOM.replace("Codex Desktop", "Chrome"),
        CUSTOM.replace("unknown", "unknown injected"),
        CUSTOM.replace("unknown", "unknown💥"),
        format!("{CUSTOM} trailing"),
        "x".repeat(513),
    ] {
        assert!(
            CodexWireProfile::parse_user_agent(&value).is_err(),
            "{value:?}"
        );
    }
    assert_eq!(
        CodexWireProfile::parse_user_agent(&format!("{CUSTOM}\n")).unwrap_err(),
        CodexWireProfileError::Unsafe,
    );
}

#[test]
fn supported_systems_and_architectures_round_trip() {
    for (os, arch) in [
        ("Mac OS", "arm64"),
        ("Mac OS", "x86_64"),
        ("Windows", "x86_64"),
        ("Windows", "arm64"),
        ("Linux", "x86_64"),
        ("Linux", "aarch64"),
    ] {
        let value = CUSTOM.replace("Mac OS", os).replace("arm64", arch);
        assert_eq!(
            CodexWireProfile::parse_user_agent(&value)
                .unwrap()
                .user_agent(),
            value
        );
    }
}

#[test]
fn custom_selection_survives_default_release_updates_then_restores_latest() {
    let state = CodexWireProfileState::new(wire_profile());
    state
        .apply_user_agent_override(&ProviderUserAgentOverride::Custom {
            user_agent: CUSTOM.to_owned(),
        })
        .unwrap();
    let frozen = state.snapshot();
    let release = CodexBundledReleaseProfile {
        codex_version: "0.154.0".to_owned(),
        desktop_version: "26.912.12345".to_owned(),
        desktop_build: "9012".to_owned(),
        verified_at: Utc::now(),
    };
    state.update_bundled_release(&release);
    assert_eq!(state.snapshot(), frozen);
    assert_eq!(state.default_snapshot().codex_version, "0.154.0");
    assert!(matches!(
        state.settings_snapshot().mode,
        CodexWireProfileOverride::Custom(_)
    ));
    state
        .apply_user_agent_override(&ProviderUserAgentOverride::Default)
        .unwrap();
    assert_eq!(state.snapshot().codex_version, "0.154.0");
    assert_eq!(state.snapshot().desktop_version, "26.912.12345");
    assert_eq!(
        frozen.user_agent(),
        CUSTOM,
        "in-flight snapshots stay unchanged"
    );
}

#[test]
fn custom_mode_is_explicit_even_when_text_matches_default() {
    let state = CodexWireProfileState::new(CodexWireProfile::parse_user_agent(CUSTOM).unwrap());
    state
        .apply_user_agent_override(&ProviderUserAgentOverride::Custom {
            user_agent: CUSTOM.to_owned(),
        })
        .unwrap();
    let settings = state.settings_snapshot();
    assert_eq!(settings.default_user_agent, settings.effective_user_agent);
    assert!(matches!(settings.mode, CodexWireProfileOverride::Custom(_)));
}

#[test]
fn invalid_custom_save_preserves_current_selection_and_residency() {
    let mut original = wire_profile();
    original.residency = Some(provider_openai::transport::profile::CodexResidency::Us);
    let state = CodexWireProfileState::new(original);
    state
        .apply_user_agent_override(&ProviderUserAgentOverride::Custom {
            user_agent: CUSTOM.to_owned(),
        })
        .unwrap();
    let before = state.snapshot();
    assert!(
        state
            .apply_user_agent_override(&ProviderUserAgentOverride::Custom {
                user_agent: "bad".to_owned(),
            })
            .is_err()
    );
    assert_eq!(state.snapshot(), before);
    assert_eq!(
        state.snapshot().residency,
        Some(provider_openai::transport::profile::CodexResidency::Us)
    );
}

#[test]
fn qx_user_agent_round_trips_cli_grammar_and_companion_fields() {
    for value in [
        qx::DEFAULT_USER_AGENT,
        "codex_cli_rs/0.146.0 (Linux 6.8.0; aarch64) unknown",
        "codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color (codex-tui; 0.146.0)",
        "codex_cli_rs/0.147.0-alpha.6 (Mac OS 15.7.1; arm64) Terminal/1.0 (codex_cli_rs; 0.147.0-alpha.6)",
    ] {
        let profile = CodexWireProfile::parse_qx_user_agent(value, &wire_profile()).unwrap();
        assert_eq!(profile.user_agent(), value);
        assert_eq!(profile.raw_user_agent.as_deref(), Some(value));
        let (product, _) = value.split_once(" (").unwrap();
        assert_eq!(
            format!("{}/{}", profile.originator, profile.codex_version),
            product
        );
        assert!(CodexWireProfile::parse_user_agent(value).is_err());
    }
    let cpr = CodexWireProfile::parse_user_agent(CUSTOM).unwrap();
    assert_eq!(cpr.raw_user_agent, None);
}

#[test]
fn qx_parser_rejects_unsafe_unknown_and_incoherent_cli_values() {
    for value in [
        CUSTOM.to_owned(),
        qx::DEFAULT_USER_AGENT.replace("codex-tui", "Chrome"),
        qx::DEFAULT_USER_AGENT.replace("0.146.0", "latest"),
        qx::DEFAULT_USER_AGENT.replace("x86_64", "windows"),
        qx::DEFAULT_USER_AGENT.replace("Ubuntu", "unknown"),
        qx::DEFAULT_USER_AGENT.replace("22.4.0", "release"),
        qx::DEFAULT_USER_AGENT.replace("xterm-256color", "xterm injected"),
        format!("{} (codex_cli_rs; 0.146.0)", qx::DEFAULT_USER_AGENT),
        format!("{} (codex-tui; 0.145.0)", qx::DEFAULT_USER_AGENT),
        format!("{} (codex-tui; 0.146.0) trailing", qx::DEFAULT_USER_AGENT),
        format!("{}\r\nX-Injected: true", qx::DEFAULT_USER_AGENT),
        format!("{}\t", qx::DEFAULT_USER_AGENT),
        format!("{}💥", qx::DEFAULT_USER_AGENT),
        format!(" {}", qx::DEFAULT_USER_AGENT),
        "x".repeat(513),
    ] {
        assert!(
            CodexWireProfile::parse_qx_user_agent(&value, &wire_profile()).is_err(),
            "{value:?}"
        );
    }
}

#[test]
fn exec_user_agent_retains_exact_bytes_and_matching_companion_headers() {
    // Public catalog v0.156.1, asset 584069027; these rows are not artifact verification.
    for value in [
        EXEC,
        "codex_exec/0.156.1 (Ubuntu 24.4.0; aarch64) xterm-256color (codex_exec; 0.156.1)",
        "codex_exec/0.156.1 (Mac OS 15.7.9; arm64) xterm-256color (codex_exec; 0.156.1)",
        "codex_exec/0.156.1 (Mac OS 15.7.9; x86_64) xterm-256color (codex_exec; 0.156.1)",
        "codex_exec/0.156.1 (Windows 10.0.26100; x86_64) xterm-256color (codex_exec; 0.156.1)",
    ] {
        let profile = CodexWireProfile::parse_custom_user_agent(value, &wire_profile()).unwrap();
        assert_eq!(profile.raw_user_agent.as_deref(), Some(value));
        assert_eq!(profile.user_agent().as_bytes(), value.as_bytes());
        assert_eq!(profile.originator, "codex_exec");
        assert_eq!(profile.codex_version, "0.156.1");
        assert_eq!(profile.verified_at, chrono::DateTime::<Utc>::UNIX_EPOCH);
        let headers = provider_openai::transport::headers::build_codex_model_headers(
            &profile, "fixture", None,
        )
        .unwrap();
        assert_eq!(headers["user-agent"].as_bytes(), value.as_bytes());
        assert_eq!(headers["originator"], "codex_exec");
        assert_eq!(headers["version"], "0.156.1");
    }
    // Grammar fixtures also preserve optional suffixes and full semver bytes.
    for value in [
        "codex_exec/0.156.1 (Linux 6.8.0; aarch64) unknown",
        "codex_exec/0.156.1-alpha.1+build.2 (Mac OS 15.7.9; arm64) Terminal/1.0 (codex_exec; 0.156.1-alpha.1+build.2)",
    ] {
        let profile = CodexWireProfile::parse_qx_user_agent(value, &wire_profile()).unwrap();
        assert_eq!(profile.user_agent(), value);
    }
}

#[test]
fn exec_parser_rejects_incomplete_unsafe_and_conflicting_identity() {
    for value in [
        "codex_exec/0.156.1".to_owned(),
        "codex_exec/0.156.1 (Ubuntu 24.4.0; x86_64)".to_owned(),
        EXEC.replace("codex_exec", "codex-exec"),
        EXEC.replace("codex_exec", "codex_exec_extra"),
        EXEC.replace("codex_exec", "CODEX_EXEC"),
        EXEC.replacen("0.156.1", "latest", 1),
        EXEC.replace("(codex_exec; 0.156.1)", "(codex-tui; 0.156.1)"),
        EXEC.replace("(codex_exec; 0.156.1)", "(codex_exec; 0.156.2)"),
        EXEC.replace("(codex_exec; 0.156.1)", "(codex_exec; 0.156.1+build)"),
        EXEC.replace("x86_64", "windows"),
        EXEC.replace("Ubuntu", "Debian"),
        EXEC.replace("24.4.0", "latest"),
        EXEC.replace("xterm-256color", "xterm extra"),
        EXEC.replace("xterm-256color", ""),
        EXEC.replace("; x86_64", ";  x86_64"),
        format!("{EXEC} trailing"),
        format!(" {EXEC}"),
        format!("{EXEC} "),
        format!("{EXEC}\r\nX-Injected: true"),
        format!("{EXEC}\0"),
        format!("{EXEC}\t"),
        format!("{EXEC}\u{7f}"),
        format!("{EXEC}\u{80}"),
    ] {
        assert!(
            CodexWireProfile::parse_custom_user_agent(&value, &wire_profile()).is_err(),
            "{value:?}"
        );
    }
}

#[test]
fn cli_and_exec_parsers_keep_the_512_byte_storage_boundary() {
    for originator in ["codex-tui", "codex_cli_rs", "codex_exec"] {
        let version = format!("0.156.1-{}", "a".repeat(180));
        let frame =
            format!("{originator}/{version} (Ubuntu 24.4.0; x86_64)  ({originator}; {version})");
        let terminal = "x".repeat(512 - frame.len());
        let value = frame.replacen(")  (", &format!(") {terminal} ("), 1);
        assert_eq!(value.len(), 512);
        assert_eq!(
            CodexWireProfile::parse_custom_user_agent(&value, &wire_profile())
                .unwrap()
                .user_agent(),
            value
        );
        let oversized = value.replacen(") ", ") x", 1);
        assert_eq!(oversized.len(), 513);
        assert_eq!(
            CodexWireProfile::parse_custom_user_agent(&oversized, &wire_profile()).unwrap_err(),
            CodexWireProfileError::Unsafe
        );
    }
}

#[test]
fn cli_selection_is_explicit_and_survives_default_release_updates() {
    {
        let user_agent = qx::DEFAULT_USER_AGENT.to_owned();
        let state = CodexWireProfileState::new(wire_profile());
        let selection = ProviderUserAgentOverride::Custom {
            user_agent: user_agent.clone(),
        };
        state.apply_user_agent_override(&selection).unwrap();
        let frozen = state.snapshot();
        assert_eq!(frozen.user_agent(), qx::DEFAULT_USER_AGENT);
        assert_eq!(frozen.originator, "codex-tui");
        assert_eq!(frozen.codex_version, "0.146.0");
        assert_eq!(state.desktop_snapshot(), state.default_snapshot());
        state.update_bundled_release(&CodexBundledReleaseProfile {
            codex_version: "0.154.0".to_owned(),
            desktop_version: "26.912.12345".to_owned(),
            desktop_build: "9012".to_owned(),
            verified_at: Utc::now(),
        });
        assert_eq!(state.snapshot(), frozen);
        assert_eq!(state.desktop_snapshot().desktop_version, "26.912.12345");
        assert_eq!(
            state.settings_snapshot().effective_desktop_version,
            "26.912.12345"
        );
        assert_eq!(
            state.settings_snapshot().effective_desktop_user_agent,
            state.default_snapshot().desktop_user_agent()
        );
        assert!(matches!(
            state.settings_snapshot().mode,
            CodexWireProfileOverride::Custom(actual) if actual.user_agent() == user_agent
        ));
        state
            .apply_user_agent_override(&ProviderUserAgentOverride::Default)
            .unwrap();
        assert_eq!(state.snapshot().codex_version, "0.154.0");
    }
}

#[test]
fn blank_custom_and_invalid_save_do_not_change_selection() {
    let mut original = wire_profile();
    original.residency = Some(provider_openai::transport::profile::CodexResidency::Us);
    let state = CodexWireProfileState::new(original.clone());
    for value in ["", "   "] {
        assert!(
            state
                .apply_user_agent_override(&ProviderUserAgentOverride::Custom {
                    user_agent: value.to_owned(),
                })
                .is_err()
        );
        assert!(matches!(
            state.settings_snapshot().mode,
            CodexWireProfileOverride::Default
        ));
        assert_eq!(state.snapshot().residency, original.residency);
    }
    let before = state.settings_snapshot();
    for selection in [
        ProviderUserAgentOverride::Custom {
            user_agent: "Chrome/123".to_owned(),
        },
        ProviderUserAgentOverride::Custom {
            user_agent: "invalid".to_owned(),
        },
        ProviderUserAgentOverride::Custom {
            user_agent: "\r\n".to_owned(),
        },
    ] {
        assert!(state.apply_user_agent_override(&selection).is_err());
        assert_eq!(state.settings_snapshot(), before);
    }
}

#[test]
fn frozen_qx_state_keeps_its_desktop_surface_and_selection_independent() {
    let state = CodexWireProfileState::new(wire_profile());
    state
        .apply_user_agent_override(&ProviderUserAgentOverride::Custom {
            user_agent: provider_openai::transport::profile::qx::DEFAULT_USER_AGENT.to_owned(),
        })
        .unwrap();
    let frozen = state.frozen();
    let settings = frozen.settings_snapshot();
    let desktop = frozen.desktop_snapshot();
    assert_eq!(desktop, wire_profile());
    assert_eq!(frozen.snapshot().user_agent(), qx::DEFAULT_USER_AGENT);
    state.update_bundled_release(&CodexBundledReleaseProfile {
        codex_version: "0.154.0".to_owned(),
        desktop_version: "26.912.12345".to_owned(),
        desktop_build: "9012".to_owned(),
        verified_at: Utc::now(),
    });
    state
        .apply_user_agent_override(&ProviderUserAgentOverride::Default)
        .unwrap();
    assert_eq!(frozen.settings_snapshot(), settings);
    assert_eq!(frozen.desktop_snapshot(), desktop);
    frozen
        .apply_user_agent_override(&ProviderUserAgentOverride::Default)
        .unwrap();
    assert_eq!(frozen.snapshot(), wire_profile());
    assert_eq!(state.snapshot().codex_version, "0.154.0");
}

#[test]
fn unified_selection_parses_desktop_and_cli_with_coherent_auxiliary_surface() {
    for ua in [
        CUSTOM,
        qx::DEFAULT_USER_AGENT,
        "codex_cli_rs/0.147.0 (Linux 6.8.0; x86_64) unknown (codex_cli_rs; 0.147.0)",
        EXEC,
    ] {
        let state = CodexWireProfileState::new(wire_profile());
        let desktop = state.desktop_snapshot();
        state
            .apply_user_agent_override(&ProviderUserAgentOverride::Custom {
                user_agent: ua.to_owned(),
            })
            .unwrap();
        let effective = state.snapshot();
        assert_eq!(effective.user_agent(), ua);
        assert_eq!(
            format!("{}/{}", effective.originator, effective.codex_version),
            ua.split_once(" (").unwrap().0
        );
        assert_eq!(
            state.desktop_snapshot(),
            if ua == CUSTOM { effective } else { desktop }
        );
    }
}

#[test]
fn unified_default_release_updates_only_default_ua_and_frozen_snapshots_stay_coherent() {
    for user_agent in [
        None,
        Some(CUSTOM.to_owned()),
        Some(qx::DEFAULT_USER_AGENT.to_owned()),
        Some(EXEC.to_owned()),
    ] {
        let state = CodexWireProfileState::new(wire_profile());
        state
            .apply_user_agent_override(
                &user_agent
                    .clone()
                    .map_or(ProviderUserAgentOverride::Default, |user_agent| {
                        ProviderUserAgentOverride::Custom { user_agent }
                    }),
            )
            .unwrap();
        let frozen = state.frozen();
        let before = state.settings_snapshot();
        state.update_bundled_release(&CodexBundledReleaseProfile {
            codex_version: "0.154.0".to_owned(),
            desktop_version: "26.912.12345".to_owned(),
            desktop_build: "9012".to_owned(),
            verified_at: Utc::now(),
        });
        assert_eq!(frozen.settings_snapshot(), before);
        if let Some(ref ua) = user_agent {
            assert_eq!(state.snapshot(), before.effective_profile);
            assert_eq!(state.snapshot().user_agent(), *ua);
        } else {
            assert_eq!(state.snapshot().codex_version, "0.154.0");
            assert_eq!(
                state.snapshot().user_agent(),
                state.default_snapshot().user_agent()
            );
        }
        assert_eq!(
            state.desktop_snapshot().desktop_version,
            if user_agent.as_deref() == Some(CUSTOM) {
                "26.901.51231"
            } else {
                "26.912.12345"
            }
        );
    }
}

#[test]
fn unified_invalid_ua_is_rejected_atomically_including_empty_custom() {
    let state = CodexWireProfileState::new(wire_profile());
    state
        .apply_user_agent_override(&ProviderUserAgentOverride::Custom {
            user_agent: provider_openai::transport::profile::qx::DEFAULT_USER_AGENT.to_owned(),
        })
        .unwrap();
    let before = state.settings_snapshot();
    for ua in [
        "",
        "   ",
        "\r\n",
        "Chrome/123",
        "codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) unknown (codex_cli_rs; 0.146.0)",
        "codex_exec/0.156.1 (Ubuntu 24.4.0; x86_64) unknown (codex_exec; 0.156.2)",
    ] {
        assert!(
            state
                .apply_user_agent_override(&ProviderUserAgentOverride::Custom {
                    user_agent: ua.to_owned(),
                })
                .is_err()
        );
        assert_eq!(state.settings_snapshot(), before);
    }
}
