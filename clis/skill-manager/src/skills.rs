//! Safe deterministic skill discovery, filtering, collision handling, and hashing.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use sha2::{Digest, Sha256};

use crate::config::fold;
use crate::domain::{ResolvedSource, SkillCandidate, SkillDiscovery, SkillState, SourceMode};
use crate::error::{Result, SkillManagerError};

/// Discover and resolve source skills using first-source-wins ordering.
///
/// # Errors
///
/// Returns an error for invalid patterns or unsafe/malformed source trees.
pub fn discover_skills(
    sources: &[ResolvedSource],
    include: &[String],
    global_exclude: &[String],
) -> Result<SkillDiscovery> {
    let include_patterns = compile_patterns(include);
    let global_exclusions = compile_patterns(global_exclude);
    let mut discovery = SkillDiscovery::default();
    for source in sources {
        let exclusions = compile_patterns(&source.entry.exclude);
        for path in detect_skill_dirs(source)? {
            let name = skill_name(&path)?;
            let folded = fold(&name);
            if !include_patterns.is_empty()
                && !include_patterns
                    .iter()
                    .any(|pattern| pattern.matches(&folded))
            {
                continue;
            }
            if global_exclusions
                .iter()
                .chain(exclusions.iter())
                .any(|pattern| pattern.matches(&folded))
            {
                continue;
            }
            let candidate = SkillCandidate {
                name,
                path,
                source: source.clone(),
            };
            if let Some(winner) = discovery.winners.get(&folded).cloned() {
                discovery
                    .collisions
                    .entry(folded)
                    .or_insert_with(|| vec![winner])
                    .push(candidate);
            } else {
                discovery.winners.insert(folded, candidate);
            }
        }
    }
    Ok(discovery)
}

/// Detect skill directories immediately below a source root.
///
/// # Errors
///
/// Returns an error when the source is missing, unreadable, or unsafe.
pub fn detect_skill_dirs(source: &ResolvedSource) -> Result<Vec<PathBuf>> {
    if !source.path.exists() {
        return Err(SkillManagerError::NotFound {
            kind: "source directory",
            reference: source.path.display().to_string(),
        });
    }
    let canonical = source
        .path
        .canonicalize()
        .map_err(|error| SkillManagerError::io(&source.path, error))?;
    validate_root(&canonical)?;
    if source.entry.mode == SourceMode::Single || canonical.join("SKILL.md").is_file() {
        if !canonical.join("SKILL.md").is_file() {
            return Err(SkillManagerError::InvalidInput(format!(
                "single-skill source is missing SKILL.md: {}",
                canonical.display()
            )));
        }
        validate_skill_tree(&canonical)?;
        return Ok(vec![canonical]);
    }
    let mut found = Vec::new();
    for entry_result in
        fs::read_dir(&canonical).map_err(|error| SkillManagerError::io(&canonical, error))?
    {
        let entry = entry_result.map_err(|error| SkillManagerError::io(&canonical, error))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| SkillManagerError::io(entry.path(), error))?;
        if metadata.file_type().is_symlink() {
            return Err(SkillManagerError::InvalidInput(format!(
                "source contains a linked skill entry: {}",
                entry.path().display()
            )));
        }
        if metadata.is_dir() && entry.path().join("SKILL.md").is_file() {
            validate_skill_tree(&entry.path())?;
            found.push(entry.path());
        }
    }
    found.sort_by(|left, right| {
        let left_name = left
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("");
        let right_name = right
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("");
        fold(left_name)
            .cmp(&fold(right_name))
            .then_with(|| left_name.cmp(right_name))
    });
    Ok(found)
}

/// Validate one complete skill tree without following links.
///
/// # Errors
///
/// Returns an error for missing manifests, links, hard links, special entries, or I/O.
pub fn validate_skill_tree(root: &Path) -> Result<()> {
    if !root.join("SKILL.md").is_file() {
        return Err(SkillManagerError::InvalidInput(format!(
            "skill is missing SKILL.md: {}",
            root.display()
        )));
    }
    let _name = skill_name(root)?;
    for item in walkdir::WalkDir::new(root).follow_links(false) {
        let item = item.map_err(|error| SkillManagerError::InvalidInput(error.to_string()))?;
        let metadata = fs::symlink_metadata(item.path())
            .map_err(|error| SkillManagerError::io(item.path(), error))?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() || !(file_type.is_dir() || file_type.is_file()) {
            return Err(SkillManagerError::InvalidInput(format!(
                "skill contains a link or special entry: {}",
                item.path().display()
            )));
        }
        if file_type.is_file() && link_count(&metadata) > 1 {
            return Err(SkillManagerError::InvalidInput(format!(
                "skill contains a hard-linked file: {}",
                item.path().display()
            )));
        }
    }
    Ok(())
}

fn validate_root(root: &Path) -> Result<()> {
    let metadata = fs::metadata(root).map_err(|error| SkillManagerError::io(root, error))?;
    if !metadata.is_dir() {
        return Err(SkillManagerError::InvalidInput(format!(
            "source root is not a directory: {}",
            root.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn link_count(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink()
}

#[cfg(windows)]
const fn link_count(_metadata: &fs::Metadata) -> u64 {
    // Stable std does not expose a Windows link count. Windows reparse points are
    // still rejected via `symlink_metadata`; ordinary files are copied to fresh
    // inodes during deployment.
    1
}

#[cfg(not(any(unix, windows)))]
const fn link_count(_metadata: &fs::Metadata) -> u64 {
    1
}

/// Validate and return the portable name of a skill directory.
///
/// # Errors
///
/// Returns an error when the name is not UTF-8 or portable.
pub fn skill_name(path: &Path) -> Result<String> {
    let raw = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| {
            SkillManagerError::InvalidInput(format!(
                "skill name is not valid UTF-8: {}",
                path.display()
            ))
        })?;
    validate_skill_name(raw)?;
    Ok(raw.to_owned())
}

/// Validate a portable single-component skill name.
///
/// # Errors
///
/// Returns an error when the name is unsafe on a supported platform.
pub fn validate_skill_name(name: &str) -> Result<()> {
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|character| character.is_control() || r#"<>:"/\|?*"#.contains(character))
        || name.len() > 255
        || name.encode_utf16().count() > 255
    {
        return Err(SkillManagerError::InvalidInput(format!(
            "skill name is not portable: {name:?}"
        )));
    }
    let stem = name.split('.').next().unwrap_or(name);
    let folded = fold(stem);
    let reserved = matches!(folded.as_str(), "con" | "prn" | "aux" | "nul")
        || (folded.len() == 4
            && (folded.starts_with("com") || folded.starts_with("lpt"))
            && matches!(folded.as_bytes()[3], b'1'..=b'9'));
    if reserved {
        return Err(SkillManagerError::InvalidInput(format!(
            "skill name is reserved on Windows: {name:?}"
        )));
    }
    Ok(())
}

/// Compare regular file paths and SHA-256 content, ignoring metadata and empty dirs.
///
/// # Errors
///
/// Returns an error for unsafe entries or filesystem reads.
pub fn directories_equal(left: &Path, right: &Path) -> Result<bool> {
    if !left.is_dir() || !right.is_dir() {
        return Ok(false);
    }
    Ok(directory_hashes(left)? == directory_hashes(right)?)
}

fn directory_hashes(root: &Path) -> Result<BTreeMap<String, [u8; 32]>> {
    let mut hashes = BTreeMap::new();
    for item in walkdir::WalkDir::new(root)
        .sort_by_file_name()
        .follow_links(false)
    {
        let item = item.map_err(|error| SkillManagerError::InvalidInput(error.to_string()))?;
        let metadata = fs::symlink_metadata(item.path())
            .map_err(|error| SkillManagerError::io(item.path(), error))?;
        if metadata.file_type().is_symlink()
            || !(metadata.file_type().is_file() || metadata.file_type().is_dir())
        {
            return Err(SkillManagerError::InvalidInput(format!(
                "skill tree contains a link or special entry: {}",
                item.path().display()
            )));
        }
        if !metadata.is_file() {
            continue;
        }
        if link_count(&metadata) > 1 {
            return Err(SkillManagerError::InvalidInput(format!(
                "skill tree contains a hard-linked file: {}",
                item.path().display()
            )));
        }
        let relative = item.path().strip_prefix(root).map_err(|error| {
            SkillManagerError::InvalidInput(format!("invalid skill path: {error}"))
        })?;
        let relative = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let bytes =
            fs::read(item.path()).map_err(|error| SkillManagerError::io(item.path(), error))?;
        hashes.insert(relative, Sha256::digest(bytes).into());
    }
    Ok(hashes)
}

/// Compute one deployment status.
///
/// # Errors
///
/// Returns an error while securely hashing either directory.
pub fn skill_state(source: Option<&Path>, target_root: &Path, skill: &str) -> Result<SkillState> {
    let deployment = target_root.join(skill);
    match (source, deployment.is_dir()) {
        (Some(source_path), true) => {
            if directories_equal(source_path, &deployment)? {
                Ok(SkillState::UpToDate)
            } else {
                Ok(SkillState::NeedsUpdate)
            }
        }
        (Some(_) | None, false) => Ok(SkillState::NotLoaded),
        (None, true) => Ok(SkillState::NoConnection),
    }
}

/// Enumerate valid deployed skill directory names.
///
/// # Errors
///
/// Returns an error when the target directory cannot be read.
pub fn deployed_skills(target_root: &Path) -> Result<IndexMap<String, String>> {
    let mut items = Vec::new();
    if !target_root.exists() {
        return Ok(IndexMap::new());
    }
    for entry_result in
        fs::read_dir(target_root).map_err(|error| SkillManagerError::io(target_root, error))?
    {
        let entry = entry_result.map_err(|error| SkillManagerError::io(target_root, error))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| SkillManagerError::io(entry.path(), error))?;
        if metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && entry.path().join("SKILL.md").is_file()
            && validate_skill_tree(&entry.path()).is_ok()
            && let Ok(name) = skill_name(&entry.path())
        {
            items.push((fold(&name), name));
        }
    }
    items.sort();
    Ok(items.into_iter().collect())
}

/// Test a folded value against Python-fnmatch-compatible patterns.
///
/// # Errors
///
/// Returns an error for an invalid pattern.
pub fn matches_patterns(value: &str, patterns: &[String]) -> Result<bool> {
    if patterns.is_empty() {
        return Ok(true);
    }
    let folded = fold(value);
    Ok(compile_patterns(patterns)
        .iter()
        .any(|pattern| pattern.matches(&folded)))
}

/// Return whether an operand uses Python-fnmatch metacharacters.
///
/// A bare `[` deliberately counts as a pattern. Python fnmatch treats an
/// unmatched opening bracket literally, but at the command boundary it still
/// means the operand is intended to select skills rather than name a source.
#[must_use]
pub fn is_fnmatch_operand(operand: &str) -> bool {
    operand.contains(['*', '?', '['])
}

/// Split `load`/`update` positional operands by their command-boundary role.
///
/// Literal values retain the historical source-reference meaning. Values with
/// fnmatch metacharacters are skill selection patterns and must not be treated
/// as filesystem paths or GitHub references.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SyncOperandSelection {
    /// Literal source references.
    pub sources: Vec<String>,
    /// Skill-name fnmatch patterns.
    pub skill_patterns: Vec<String>,
}

/// Split positional operands into literal source references and skill patterns.
#[must_use]
pub fn split_sync_operands(operands: &[String]) -> SyncOperandSelection {
    let mut selection = SyncOperandSelection::default();
    for operand in operands {
        if is_fnmatch_operand(operand) {
            selection.skill_patterns.push(operand.clone());
        } else {
            selection.sources.push(operand.clone());
        }
    }
    selection
}

/// The result of expanding positional fnmatch patterns over a candidate universe.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PatternExpansion {
    /// Candidate names selected by at least one supplied pattern, in universe order.
    pub matched: Vec<String>,
    /// Patterns that selected no candidate, in argument order.
    pub unmatched_patterns: Vec<String>,
}

/// Expand positional patterns against a supplied candidate universe.
///
/// Pattern operands are OR-combined. The caller can then AND [`PatternExpansion::matched`]
/// with its existing filter selection. An empty pattern set selects all candidates.
/// This helper intentionally does not interpret a candidate as a path or source.
///
/// # Errors
///
/// Returns an error only when the existing fnmatch matcher rejects a pattern.
pub fn expand_skill_patterns<'a>(
    patterns: &[String],
    universe: impl IntoIterator<Item = &'a str>,
) -> Result<PatternExpansion> {
    let candidates = universe
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if patterns.is_empty() {
        return Ok(PatternExpansion {
            matched: candidates,
            unmatched_patterns: Vec::new(),
        });
    }

    let mut matched = Vec::new();
    for candidate in &candidates {
        if matches_patterns(candidate, patterns)? {
            matched.push(candidate.clone());
        }
    }
    let mut unmatched_patterns = Vec::new();
    for pattern in patterns {
        let one_pattern = std::slice::from_ref(pattern);
        let mut any_match = false;
        for candidate in &candidates {
            if matches_patterns(candidate, one_pattern)? {
                any_match = true;
                break;
            }
        }
        if !any_match {
            unmatched_patterns.push(pattern.clone());
        }
    }
    Ok(PatternExpansion {
        matched,
        unmatched_patterns,
    })
}

fn compile_patterns(patterns: &[String]) -> Vec<FnPattern> {
    patterns
        .iter()
        .map(|pattern| FnPattern::parse(&fold(pattern)))
        .collect()
}

#[derive(Clone, Debug)]
struct FnPattern(Vec<PatternToken>);

#[derive(Clone, Debug)]
enum PatternToken {
    Star,
    Any,
    Literal(char),
    Class {
        negated: bool,
        members: Vec<ClassMember>,
    },
}

#[derive(Clone, Copy, Debug)]
enum ClassMember {
    Single(char),
    Range(char, char),
}

impl FnPattern {
    fn parse(pattern: &str) -> Self {
        let characters: Vec<char> = pattern.chars().collect();
        let mut tokens = Vec::new();
        let mut index = 0;
        while index < characters.len() {
            match characters[index] {
                '*' => {
                    if !matches!(tokens.last(), Some(PatternToken::Star)) {
                        tokens.push(PatternToken::Star);
                    }
                    index += 1;
                }
                '?' => {
                    tokens.push(PatternToken::Any);
                    index += 1;
                }
                '[' => {
                    if let Some((token, next)) = parse_class(&characters, index) {
                        tokens.push(token);
                        index = next;
                    } else {
                        tokens.push(PatternToken::Literal('['));
                        index += 1;
                    }
                }
                literal => {
                    tokens.push(PatternToken::Literal(literal));
                    index += 1;
                }
            }
        }
        Self(tokens)
    }

    fn matches(&self, value: &str) -> bool {
        let characters: Vec<char> = value.chars().collect();
        let width = characters.len() + 1;
        let mut memo = vec![None; (self.0.len() + 1) * width];
        matches_from(&self.0, &characters, 0, 0, width, &mut memo)
    }
}

fn parse_class(characters: &[char], start: usize) -> Option<(PatternToken, usize)> {
    let mut end = start + 1;
    if characters.get(end) == Some(&'!') {
        end += 1;
    }
    if characters.get(end) == Some(&']') {
        end += 1;
    }
    while characters
        .get(end)
        .is_some_and(|character| *character != ']')
    {
        end += 1;
    }
    if end >= characters.len() {
        return None;
    }
    let mut content = &characters[start + 1..end];
    let negated = content.first() == Some(&'!');
    if negated {
        content = &content[1..];
    }
    let mut members = Vec::new();
    let mut index = 0;
    while index < content.len() {
        if index + 2 < content.len() && content[index + 1] == '-' {
            members.push(ClassMember::Range(content[index], content[index + 2]));
            index += 3;
        } else {
            members.push(ClassMember::Single(content[index]));
            index += 1;
        }
    }
    Some((PatternToken::Class { negated, members }, end + 1))
}

fn matches_from(
    pattern: &[PatternToken],
    value: &[char],
    pattern_index: usize,
    value_index: usize,
    width: usize,
    memo: &mut [Option<bool>],
) -> bool {
    let memo_index = pattern_index * width + value_index;
    if let Some(result) = memo[memo_index] {
        return result;
    }
    let result = match pattern.get(pattern_index) {
        None => value_index == value.len(),
        Some(PatternToken::Star) => {
            matches_from(pattern, value, pattern_index + 1, value_index, width, memo)
                || value_index < value.len()
                    && matches_from(pattern, value, pattern_index, value_index + 1, width, memo)
        }
        Some(token) if value_index < value.len() => {
            token_matches(token, value[value_index])
                && matches_from(
                    pattern,
                    value,
                    pattern_index + 1,
                    value_index + 1,
                    width,
                    memo,
                )
        }
        Some(_) => false,
    };
    memo[memo_index] = Some(result);
    result
}

fn token_matches(token: &PatternToken, character: char) -> bool {
    match token {
        PatternToken::Any => true,
        PatternToken::Literal(expected) => *expected == character,
        PatternToken::Class { negated, members } => {
            let contains = members.iter().any(|member| match member {
                ClassMember::Single(expected) => *expected == character,
                ClassMember::Range(start, end) => *start <= character && character <= *end,
            });
            contains != *negated
        }
        PatternToken::Star => false,
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::detect_skill_dirs;
    use super::{
        directories_equal, expand_skill_patterns, is_fnmatch_operand, matches_patterns,
        split_sync_operands, validate_skill_name,
    };
    #[cfg(unix)]
    use crate::config::source_from_reference;
    #[cfg(unix)]
    use crate::domain::{ResolvedSource, SourceMode};

    #[test]
    fn rejects_portability_hazards() {
        for name in ["CON", "lpt1.txt", "bad.", "a/b"] {
            assert!(validate_skill_name(name).is_err(), "{name}");
        }
        assert!(validate_skill_name("creating-a-plan").is_ok());
    }

    #[test]
    fn directory_equality_ignores_empty_directories() {
        let left = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let right = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(left.path().join("SKILL.md"), "same")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(right.path().join("SKILL.md"), "same")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::create_dir(left.path().join("empty"))
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(
            directories_equal(left.path(), right.path())
                .unwrap_or_else(|error| unreachable!("{error}"))
        );
    }

    #[test]
    fn python_fnmatch_supports_classes_negation_and_literal_unmatched_brackets() {
        assert!(
            matches_patterns("[draft", &["[draft".into()])
                .unwrap_or_else(|error| unreachable!("{error}"))
        );
        assert!(
            matches_patterns("beta", &["[!a]eta".into()])
                .unwrap_or_else(|error| unreachable!("{error}"))
        );
        assert!(
            !matches_patterns("aeta", &["[!a]eta".into()])
                .unwrap_or_else(|error| unreachable!("{error}"))
        );
        assert!(
            matches_patterns("skill-7", &["skill-[0-9]".into()])
                .unwrap_or_else(|error| unreachable!("{error}"))
        );
        assert!(
            matches_patterns("STRASSE-α", &["straße-?".into()])
                .unwrap_or_else(|error| unreachable!("{error}"))
        );
    }

    #[test]
    fn sync_operands_keep_literal_source_references_and_extract_patterns() {
        let selection = split_sync_operands(&[
            "owner/repo".into(),
            "./local".into(),
            "grill-*".into(),
            "[ab]eta".into(),
            "one?".into(),
        ]);
        assert_eq!(selection.sources, ["owner/repo", "./local"]);
        assert_eq!(selection.skill_patterns, ["grill-*", "[ab]eta", "one?"]);
        assert!(!is_fnmatch_operand("owner/repo"));
        assert!(is_fnmatch_operand("[literal"));
    }

    #[test]
    fn expanding_patterns_is_or_combined_and_accounts_for_unmatched_patterns() {
        let patterns = vec!["grill-*".into(), "beta".into(), "missing-*".into()];
        let universe = ["alpha", "grill-me", "beta", "grill-plan"];
        let expanded = expand_skill_patterns(&patterns, universe)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(expanded.matched, ["grill-me", "beta", "grill-plan"]);
        assert_eq!(expanded.unmatched_patterns, ["missing-*"]);

        let all = expand_skill_patterns(&[], ["alpha", "beta"])
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(all.matched, ["alpha", "beta"]);
        assert!(all.unmatched_patterns.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn source_root_may_have_a_symlinked_ancestor() {
        use std::os::unix::fs::symlink;

        let sandbox = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let real_parent = sandbox.path().join("real");
        let source_root = real_parent.join("collection");
        let skill = source_root.join("alpha");
        std::fs::create_dir_all(&skill).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(skill.join("SKILL.md"), "# alpha")
            .unwrap_or_else(|error| unreachable!("{error}"));
        let alias = sandbox.path().join("alias");
        symlink(&real_parent, &alias).unwrap_or_else(|error| unreachable!("{error}"));

        let mut entry = source_from_reference(
            source_root
                .to_str()
                .unwrap_or_else(|| unreachable!("temporary path must be UTF-8")),
            Some(SourceMode::Collection),
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        entry.path = Some(alias.join("collection"));
        let resolved = ResolvedSource {
            entry,
            path: alias.join("collection"),
            from_cache: false,
            temporary: None,
        };

        let found = detect_skill_dirs(&resolved).unwrap_or_else(|error| unreachable!("{error}"));
        let canonical_skill = skill
            .canonicalize()
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(found, vec![canonical_skill]);
    }
}
