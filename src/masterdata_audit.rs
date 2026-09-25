//! Master data audit helpers shared by every load path that hands strings to
//! the engine (registry fetch and JSON push): key normalisation with the C++
//! bridge's rules, a cheap non-empty check for the key tables, and the
//! required/optional key diff.

use std::collections::HashMap;

use crate::registry::{OPTIONAL_MASTERDATA_KEYS, REQUIRED_MASTERDATA_KEYS};

/// Required tables that must carry at least one row for any recommend to be
/// meaningful. Strict subset of `REQUIRED_MASTERDATA_KEYS`.
/// `worldBloomSupportDeckBonuses` is deliberately absent: the engine's key
/// check requires it but never loads it, so `[]` is legitimate.
pub const KEY_MASTERDATA_TABLES: [&str; 15] = [
    "areaItemLevels",
    "areaItems",
    "areas",
    "cardEpisodes",
    "cards",
    "cardRarities",
    "characterRanks",
    "gameCharacters",
    "gameCharacterUnits",
    "honors",
    "masterLessons",
    "musicDifficulties",
    "musics",
    "musicVocals",
    "skills",
];

/// Mirror of the bridge's `normalize_masterdata_key`: basename after the last
/// `/` or `\`, then strip a case-insensitive `.json` suffix.
pub fn normalize_masterdata_key(raw: &str) -> &str {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let bytes = base.as_bytes();
    if bytes.len() >= 5 && bytes[bytes.len() - 5..].eq_ignore_ascii_case(b".json") {
        // The last five bytes are ASCII, so this is a valid char boundary.
        &base[..base.len() - 5]
    } else {
        base
    }
}

/// Canonical key -> value with the bridge's precedence: raw keys are visited in
/// sorted order (`std::map` order), a raw key that is already canonical always
/// wins, otherwise the first writer wins.
pub fn canonical_view(data: &HashMap<String, String>) -> HashMap<&str, &str> {
    let mut raw_keys: Vec<&String> = data.keys().collect();
    raw_keys.sort();
    let mut view: HashMap<&str, &str> = HashMap::with_capacity(raw_keys.len());
    for raw in raw_keys {
        let canonical = normalize_masterdata_key(raw);
        if !view.contains_key(canonical) || raw.as_str() == canonical {
            view.insert(canonical, data[raw].as_str());
        }
    }
    view
}

/// Cheap non-empty guard, never a full parse (`cards` is tens of MB): leading
/// whitespace, `[`, whitespace, then anything but `]`/EOF. Malformed input is
/// left to the engine.
pub fn is_nonempty_json_array(text: &str) -> bool {
    let Some(rest) = text.trim_start().strip_prefix('[') else {
        return false;
    };
    !matches!(rest.trim_start().chars().next(), None | Some(']'))
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MasterdataAudit {
    /// Sorted required keys absent from the data.
    pub missing_required_keys: Vec<String>,
    /// Sorted key tables that are present but not a non-empty array.
    pub empty_key_tables: Vec<String>,
    /// Sorted optional keys absent from the data.
    pub missing_optional_keys: Vec<String>,
}

/// Audits a raw key -> JSON text map after normalising keys the way the bridge does.
pub fn audit_masterdata(data: &HashMap<String, String>) -> MasterdataAudit {
    let view = canonical_view(data);
    let mut missing_required_keys: Vec<String> = REQUIRED_MASTERDATA_KEYS
        .iter()
        .filter(|key| !view.contains_key(**key))
        .map(|key| (*key).to_owned())
        .collect();
    let mut empty_key_tables: Vec<String> = KEY_MASTERDATA_TABLES
        .iter()
        .filter(|key| {
            view.get(**key)
                .is_some_and(|text| !is_nonempty_json_array(text))
        })
        .map(|key| (*key).to_owned())
        .collect();
    let mut missing_optional_keys: Vec<String> = OPTIONAL_MASTERDATA_KEYS
        .iter()
        .filter(|key| !view.contains_key(**key))
        .map(|key| (*key).to_owned())
        .collect();
    missing_required_keys.sort();
    empty_key_tables.sort();
    missing_optional_keys.sort();
    MasterdataAudit {
        missing_required_keys,
        empty_key_tables,
        missing_optional_keys,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::path::Path;

    use super::*;

    /// C7 freeze: the engine's own lists at
    /// `DECK_CPP_REF = b2387b7f09e5a420c9bfee9ece8903b345dd39cd`,
    /// `_cpp_src/src/data-provider/master-data.cpp:13-52`
    /// (`requiredMasterDataKeys`, `notRequiredMasterDataKeys`).
    const FROZEN_REQUIRED: [&str; 25] = [
        "areaItemLevels",
        "areaItems",
        "areas",
        "cardEpisodes",
        "cards",
        "cardRarities",
        "characterRanks",
        "eventCards",
        "eventDeckBonuses",
        "eventExchangeSummaries",
        "events",
        "eventItems",
        "eventRarityBonusRates",
        "gameCharacters",
        "gameCharacterUnits",
        "honors",
        "masterLessons",
        "musicDifficulties",
        "musics",
        "musicVocals",
        "shopItems",
        "skills",
        "worldBloomDifferentAttributeBonuses",
        "worldBlooms",
        "worldBloomSupportDeckBonuses",
    ];
    const FROZEN_OPTIONAL: [&str; 13] = [
        "worldBloomSupportDeckUnitEventLimitedBonuses",
        "cardMysekaiCanvasBonuses",
        "eventCardBonusLimits",
        "eventHonorBonuses",
        "eventMysekaiFixtureGameCharacterPerformanceBonusLimits",
        "eventSkillScoreUpLimits",
        "ingameCombos",
        "ingameNotes",
        "ingameNoteJudges",
        "mysekaiFixtureGameCharacterGroups",
        "mysekaiFixtureGameCharacterGroupPerformanceBonuses",
        "mysekaiGates",
        "mysekaiGateLevels",
    ];

    /// Pulls the string literals out of a `const std::vector<std::string> <name> = { ... };`
    /// initialiser in the engine source, in source order. Deliberately dumb: find
    /// `<name>`, take the first `{` after it through the matching `}`, collect every
    /// `"..."` literal (the engine's lists contain no escapes or comments). Panics if
    /// the initialiser is not found, so a rename on the C++ side is a test failure
    /// rather than a silent empty list.
    fn cpp_key_list(source: &str, name: &str) -> Vec<String> {
        let start = source
            .find(name)
            .unwrap_or_else(|| panic!("engine list {name} not found"));
        let after = &source[start + name.len()..];
        let open = after
            .find('{')
            .unwrap_or_else(|| panic!("engine list {name} has no initialiser"));
        let body = &after[open + 1..];
        let mut depth = 1usize;
        let mut end = None;
        for (idx, ch) in body.char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(idx);
                        break;
                    }
                }
                _ => {}
            }
        }
        let body = &body[..end.unwrap_or_else(|| panic!("engine list {name} is unterminated"))];
        body.split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect()
    }

    fn full_map() -> HashMap<String, String> {
        REQUIRED_MASTERDATA_KEYS
            .iter()
            .chain(OPTIONAL_MASTERDATA_KEYS.iter())
            .map(|key| {
                let body = if KEY_MASTERDATA_TABLES.contains(key) {
                    "[{}]"
                } else {
                    "[]"
                };
                ((*key).to_owned(), body.to_owned())
            })
            .collect()
    }

    #[test]
    fn key_tables_are_frozen() {
        assert_eq!(KEY_MASTERDATA_TABLES.len(), 15);
        let unique: HashSet<&str> = KEY_MASTERDATA_TABLES.iter().copied().collect();
        assert_eq!(unique.len(), 15);
        for key in KEY_MASTERDATA_TABLES {
            assert!(REQUIRED_MASTERDATA_KEYS.contains(&key), "{key}");
        }
        assert!(!KEY_MASTERDATA_TABLES.contains(&"worldBloomSupportDeckBonuses"));
        for key in ["cards", "musics", "skills"] {
            assert!(KEY_MASTERDATA_TABLES.contains(&key), "{key}");
        }
    }

    #[test]
    fn masterdata_key_lists_are_frozen() {
        assert_eq!(
            REQUIRED_MASTERDATA_KEYS.as_slice(),
            FROZEN_REQUIRED.as_slice()
        );
        assert_eq!(
            OPTIONAL_MASTERDATA_KEYS.as_slice(),
            FROZEN_OPTIONAL.as_slice()
        );
        let all: HashSet<&str> = FROZEN_REQUIRED
            .iter()
            .chain(FROZEN_OPTIONAL.iter())
            .copied()
            .collect();
        assert_eq!(all.len(), 38);
        let required: HashSet<&str> = FROZEN_REQUIRED.iter().copied().collect();
        let optional: HashSet<&str> = FROZEN_OPTIONAL.iter().copied().collect();
        assert!(required.is_disjoint(&optional));
        for key in KEY_MASTERDATA_TABLES {
            assert!(required.contains(key), "{key}");
        }
    }

    #[test]
    fn masterdata_key_lists_match_engine_source() {
        let path = Path::new(env!("DECK_CPP_SRC_DIR")).join("src/data-provider/master-data.cpp");
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(_) => {
                eprintln!("skipped: engine source not present at {}", path.display());
                return;
            }
        };
        assert_eq!(
            cpp_key_list(&source, "requiredMasterDataKeys"),
            REQUIRED_MASTERDATA_KEYS
        );
        assert_eq!(
            cpp_key_list(&source, "notRequiredMasterDataKeys"),
            // This extra table is consumed by the service bridge, not the C++ core.
            OPTIONAL_MASTERDATA_KEYS
                .into_iter()
                .filter(|key| *key != "ingameNoteJudges")
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn cpp_key_list_parses_nested_and_panics_on_missing() {
        let source = "const std::vector<std::string> a = {\n    \"x\",\n    \"y\"\n};";
        assert_eq!(cpp_key_list(source, "a"), vec!["x", "y"]);
        assert!(std::panic::catch_unwind(|| cpp_key_list(source, "missing")).is_err());
    }

    #[test]
    fn normalize_matches_bridge() {
        let cases = [
            ("cards", "cards"),
            ("cards.json", "cards"),
            ("cards.JSON", "cards"),
            ("a/b/cards.json", "cards"),
            ("a\\cards.Json", "cards"),
            ("cards.jsonx", "cards.jsonx"),
            ("x", "x"),
            ("dir/", ""),
            (".json", ""),
        ];
        for (raw, expected) in cases {
            assert_eq!(normalize_masterdata_key(raw), expected, "{raw}");
        }
    }

    #[test]
    fn canonical_key_wins_over_alias() {
        let data = HashMap::from([
            ("cards".to_owned(), "[]".to_owned()),
            ("nested/cards.JSON".to_owned(), "[{}]".to_owned()),
        ]);
        let view = canonical_view(&data);
        assert_eq!(view.len(), 1);
        assert_eq!(view["cards"], "[]");

        let alias_only = HashMap::from([("nested/cards.JSON".to_owned(), "[{}]".to_owned())]);
        assert_eq!(canonical_view(&alias_only)["cards"], "[{}]");

        // Two aliases: the first in sorted order wins.
        let aliases = HashMap::from([
            ("b/cards.json".to_owned(), "[2]".to_owned()),
            ("a/cards.json".to_owned(), "[1]".to_owned()),
        ]);
        assert_eq!(canonical_view(&aliases)["cards"], "[1]");
    }

    #[test]
    fn nonempty_array_scan() {
        for text in ["[]", "  [ ]  ", "", "{}", "   "] {
            assert!(!is_nonempty_json_array(text), "{text:?}");
        }
        for text in ["[{}]", "\n[1", "[ {\"id\":1} ]"] {
            assert!(is_nonempty_json_array(text), "{text:?}");
        }
    }

    #[test]
    fn audit_reports_all_three_lists() {
        let mut data = full_map();
        assert_eq!(audit_masterdata(&data), MasterdataAudit::default());

        data.remove("ingameNotes");
        data.remove("mysekaiGates");
        data.insert("skills".to_owned(), "[]".to_owned());
        data.insert("cards".to_owned(), "[]".to_owned());
        let audit = audit_masterdata(&data);
        assert_eq!(audit.missing_optional_keys, ["ingameNotes", "mysekaiGates"]);
        assert_eq!(audit.empty_key_tables, ["cards", "skills"]);
        assert!(audit.missing_required_keys.is_empty());

        data.remove("musics");
        let audit = audit_masterdata(&data);
        assert_eq!(audit.missing_required_keys, ["musics"]);
        assert!(!audit.empty_key_tables.contains(&"musics".to_owned()));

        // Aliased keys count as present, with the bridge's precedence.
        data.insert("master/musics.json".to_owned(), "[{}]".to_owned());
        assert!(audit_masterdata(&data).missing_required_keys.is_empty());
    }
}
