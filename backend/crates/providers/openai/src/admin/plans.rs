//! Codex 套餐列与筛选共用的显示名称及原始类型映射。

// Pro 系列保留项目短名，其余已知套餐对应现有 Codex /status 显示。
const PLANS: &[(&str, &[&str])] = &[
    ("Free", &["free", "free_workspace", "guest"]),
    ("Go", &["go"]),
    ("Plus", &["plus"]),
    ("ProLite", &["prolite"]),
    ("Pro", &["pro"]),
    ("ProMax", &["promax"]),
    ("Business", &["team", "self_serve_business_usage_based"]),
    ("Business Premium", &["self_serve_business_prolite"]),
    (
        "Enterprise",
        &[
            "business",
            "ent26",
            "enterprise",
            "hc",
            "enterprise_cbp_usage_based",
        ],
    ),
    ("Enterprise (Automation)", &["enterprise_cbp_automation"]),
    ("Edu", &["edu", "education"]),
    ("Edu Plus", &["edu_plus"]),
    ("Edu Pro", &["edu_pro"]),
    ("Academic Researchers", &["sci"]),
    ("Academic Researchers Plus", &["sci_plus"]),
];

fn find(plan_type: &str) -> Option<&'static (&'static str, &'static [&'static str])> {
    PLANS.iter().find(|(_, aliases)| {
        aliases
            .iter()
            .any(|alias| alias.eq_ignore_ascii_case(plan_type))
    })
}

pub(super) fn display_name(plan_type: &str) -> &str {
    find(plan_type).map_or(plan_type, |(name, _)| *name)
}

pub(super) fn aliases(plan_type: &str) -> Option<&'static [&'static str]> {
    find(plan_type).map(|(_, aliases)| *aliases)
}
