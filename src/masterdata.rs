use std::path::{Path, PathBuf};

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
