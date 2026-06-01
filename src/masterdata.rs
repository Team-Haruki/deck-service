use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MasterdataSignature {
    pub base_dir: String,
    pub hash: u64,
    pub file_count: usize,
}

pub fn resolve_masterdata_base_dir(base_dir: &str, region: &str) -> String {
    let trimmed_region = region.trim();
    for candidate in candidate_masterdata_dirs(base_dir, trimmed_region) {
        if has_masterdata_marker(&candidate) {
            return candidate.to_string_lossy().into_owned();
        }
    }
    base_dir.to_string()
}

fn candidate_masterdata_dirs(base_dir: &str, region: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let trimmed_base_dir = base_dir.trim();
    if !trimmed_base_dir.is_empty() {
        let base = PathBuf::from(trimmed_base_dir);
        push_candidate(&mut candidates, base.clone());
        push_candidate(&mut candidates, base.join("master"));
        if !region.is_empty() {
            let base_name_matches_region = base
                .file_name()
                .and_then(|value| value.to_str())
                .map(|value| value.eq_ignore_ascii_case(region))
                .unwrap_or(false);
            if !base_name_matches_region {
                push_candidate(&mut candidates, base.join(region));
                push_candidate(&mut candidates, base.join(region).join("master"));
            }
            for repo_dir in region_masterdata_repo_dirs(region) {
                push_candidate(&mut candidates, base.join(repo_dir));
                push_candidate(&mut candidates, base.join(repo_dir).join("master"));
            }
        }
    }

    if !region.is_empty() {
        push_candidate(&mut candidates, PathBuf::from("/data").join(region));
        push_candidate(&mut candidates, PathBuf::from("/masterdata").join(region));
        push_candidate(
            &mut candidates,
            PathBuf::from("/data").join(region).join("master"),
        );
        push_candidate(
            &mut candidates,
            PathBuf::from("/masterdata").join(region).join("master"),
        );
        for repo_dir in region_masterdata_repo_dirs(region) {
            push_candidate(&mut candidates, PathBuf::from("/data").join(repo_dir));
            push_candidate(&mut candidates, PathBuf::from("/masterdata").join(repo_dir));
            push_candidate(
                &mut candidates,
                PathBuf::from("/data").join(repo_dir).join("master"),
            );
            push_candidate(
                &mut candidates,
                PathBuf::from("/masterdata").join(repo_dir).join("master"),
            );
        }
    }

    candidates
}

fn region_masterdata_repo_dirs(region: &str) -> &'static [&'static str] {
    match region {
        "jp" => &["haruki-sekai-master"],
        "en" => &["haruki-sekai-en-master"],
        "kr" => &["haruki-sekai-kr-master"],
        "cn" => &["haruki-sekai-sc-master"],
        "tw" => &["haruki-sekai-tc-master"],
        _ => &[],
    }
}

fn push_candidate(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if candidate.as_os_str().is_empty() || candidates.iter().any(|existing| existing == &candidate)
    {
        return;
    }
    candidates.push(candidate);
}

fn has_masterdata_marker(path: &Path) -> bool {
    path.join("areaItemLevels.json").is_file()
}

pub fn masterdata_signature(
    base_dir: &str,
    region: &str,
) -> std::io::Result<Option<MasterdataSignature>> {
    let resolved_base_dir = resolve_masterdata_base_dir(base_dir, region);
    let resolved_path = PathBuf::from(resolved_base_dir.trim());
    if resolved_path.as_os_str().is_empty() || !has_masterdata_marker(&resolved_path) {
        return Ok(None);
    }

    let mut entries = Vec::new();
    collect_json_file_signatures(&resolved_path, &resolved_path, &mut entries)?;
    if entries.is_empty() {
        return Ok(None);
    }
    entries.sort();

    let mut hasher = DefaultHasher::new();
    entries.hash(&mut hasher);
    Ok(Some(MasterdataSignature {
        base_dir: resolved_path.to_string_lossy().into_owned(),
        hash: hasher.finish(),
        file_count: entries.len(),
    }))
}

fn collect_json_file_signatures(
    root: &Path,
    dir: &Path,
    entries: &mut Vec<(String, u64, u128)>,
) -> std::io::Result<()> {
    let mut children = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.path());

    for child in children {
        let path = child.path();
        let file_type = child.file_type()?;
        if file_type.is_dir() {
            if child.file_name().to_string_lossy() == ".git" {
                continue;
            }
            collect_json_file_signatures(root, &path, entries)?;
            continue;
        }
        if !file_type.is_file() || path.extension().and_then(|value| value.to_str()) != Some("json")
        {
            continue;
        }

        let metadata = child.metadata()?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path.as_path())
            .to_string_lossy()
            .replace('\\', "/");
        entries.push((relative, metadata.len(), modified));
    }

    Ok(())
}
