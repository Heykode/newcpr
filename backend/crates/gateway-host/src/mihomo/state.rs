use chrono::{DateTime, Utc};
use gateway_admin::model::{AdminError, mihomo::*};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) type Node = Map<String, Value>;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Saved {
    pub download_mode: SubscriptionDownloadMode,
    pub subscriptions: Vec<Subscription>,
    pub dynamic_proxies: Vec<String>,
    pub nodes: Vec<Node>,
    pub names: BTreeMap<String, String>,
    pub disabled: BTreeMap<String, String>,
    pub countries: BTreeMap<String, CountryObservation>,
    pub country_filter: CountryFilter,
    pub secret: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Subscription {
    pub url: String,
    pub label: String,
    pub enabled: bool,
    pub cache: Option<SubscriptionCache>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SubscriptionCache {
    pub nodes: Vec<Node>,
    pub names: BTreeMap<String, String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CountryObservation {
    pub code: Option<String>,
    pub checked_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

pub(super) fn digest(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

pub(super) fn node_id(node: &Node) -> String {
    // The workspace enables preserve_order. Canonicalize nested objects just as
    // Go's encoding/json does; labels and YAML key order are not exit identity.
    let mut identity = node.clone();
    identity.remove("name");
    let mut identity = Value::Object(identity);
    identity.sort_all_objects();
    digest(&serde_json::to_vec(&identity).expect("JSON node serialization"))
}

pub(super) fn node_name(node: &Node) -> &str {
    node.get("name").and_then(Value::as_str).unwrap_or("")
}

// Generated with Sub2API's countryCodes()/validCountry() and x/text v0.41.0.
pub(super) const COUNTRY_CODES: &str = "AC AD AE AF AG AI AL AM AN AO AQ AR AS AT AU AW AX AZ BA BB BD BE BF BG BH BI BJ BL BM BN BO BQ BR BS BT BU BV BW BY BZ CA CC CD CF CG CH CI CK CL CM CN CO CP CQ CR CS CT CU CV CW CX CY CZ DD DE DG DJ DK DM DO DY DZ EA EC EE EG EH ER ES ET EZ FI FJ FK FM FO FQ FR FX GA GB GD GE GF GG GH GI GL GM GN GP GQ GR GS GT GU GW GY HK HM HN HR HT HU HV IC ID IE IL IM IN IO IQ IR IS IT JE JM JO JP JT KE KG KH KI KM KN KP KR KW KY KZ LA LB LC LI LK LR LS LT LU LV LY MA MC MD ME MF MG MH MI MK ML MM MN MO MP MQ MR MS MT MU MV MW MX MY MZ NA NC NE NF NG NH NI NL NO NP NQ NR NT NU NZ OM PA PC PE PF PG PH PK PL PM PN PR PS PT PU PW PY PZ QA RE RH RO RS RU RW SA SB SC SD SE SG SH SI SJ SK SL SM SN SO SR SS ST SU SV SX SY SZ TA TC TD TF TG TH TJ TK TL TM TN TO TP TR TT TV TW TZ UA UG UM UN US UY UZ VA VC VD VE VG VI VN VU WF WK WS XK YD YE YT YU ZA ZM ZR ZW";

pub(super) fn normalize_country(mut filter: CountryFilter) -> Result<CountryFilter, AdminError> {
    let valid: BTreeSet<_> = COUNTRY_CODES.split_whitespace().collect();
    let mut codes = BTreeSet::new();
    for code in filter.codes {
        let code = code.trim().to_ascii_uppercase();
        if !valid.contains(code.as_str()) {
            return Err(AdminError::invalid("地区代码无效"));
        }
        codes.insert(code);
    }
    if filter.mode != CountryFilterMode::Off && codes.is_empty() {
        return Err(AdminError::invalid("请选择至少一个国家或地区"));
    }
    filter.codes = codes.into_iter().collect();
    Ok(filter)
}

impl Saved {
    pub fn dynamic_ids(&self) -> BTreeSet<String> {
        self.dynamic_proxies
            .iter()
            .filter_map(|raw| super::dynamic::parse(raw).ok())
            .map(|parsed| node_id(&parsed.node()))
            .collect()
    }

    pub fn country_allowed(&self, node: &Node, dynamic: bool) -> bool {
        let filter = &self.country_filter;
        if filter.mode == CountryFilterMode::Off {
            return true;
        }
        if dynamic {
            return filter.dynamic_provider_managed || filter.allow_unknown;
        }
        let Some(code) = self
            .countries
            .get(node_name(node))
            .and_then(|o| o.code.as_ref())
        else {
            return filter.allow_unknown;
        };
        let selected = filter.codes.contains(code);
        match filter.mode {
            CountryFilterMode::Include => selected,
            CountryFilterMode::Exclude => !selected,
            CountryFilterMode::Off => true,
        }
    }

    pub fn eligible(&self, node: &Node, dynamic: bool) -> bool {
        !self.disabled.contains_key(node_name(node)) && self.country_allowed(node, dynamic)
    }

    pub fn subscriptions_view(&self) -> Vec<MihomoSubscription> {
        self.subscriptions
            .iter()
            .enumerate()
            .map(|(index, source)| {
                let id = digest(source.url.as_bytes());
                MihomoSubscription {
                    label: if source.label.is_empty() {
                        format!("订阅 {} · {}", index + 1, &id[..8])
                    } else {
                        source.label.clone()
                    },
                    id,
                    enabled: source.enabled,
                    nodes: source.cache.as_ref().map_or(0, |c| c.nodes.len()),
                    cached: source.cache.is_some(),
                    updated_at: source.cache.as_ref().map(|c| c.updated_at),
                }
            })
            .collect()
    }
}
