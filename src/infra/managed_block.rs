use std::fs;
use std::path::Path;

use crate::error::{Error, Result};
use crate::model::FileChange;

#[derive(Debug, Clone)]
pub(crate) struct ManagedBlock {
    pub(crate) start_marker: String,
    pub(crate) end_marker: String,
    pub(crate) body: String,
}

impl ManagedBlock {
    pub(crate) fn render(&self) -> String {
        format!(
            "{}\n{}\n{}\n",
            self.start_marker,
            self.body.trim_end(),
            self.end_marker
        )
    }
}

pub(crate) fn upsert(path: &Path, block: &ManagedBlock) -> Result<FileChange> {
    let original = read_utf8_file(path)?;
    let rewritten = rewrite(
        path,
        original.as_deref().unwrap_or_default(),
        block,
        RewriteMode::Upsert,
    )?;
    let updated = if rewritten.found {
        rewritten.contents
    } else {
        append_block(original.as_deref().unwrap_or_default(), block)
    };

    if original.as_deref() == Some(updated.as_str()) {
        return Ok(FileChange::Unchanged);
    }

    let parent = path.parent().ok_or_else(|| Error::PathHasNoParent {
        path: path.to_path_buf(),
    })?;
    fs::create_dir_all(parent)
        .map_err(|source| Error::io("create parent directory for", parent, source))?;
    fs::write(path, updated).map_err(|source| Error::io("write file", path, source))?;

    Ok(if original.is_some() || rewritten.found {
        FileChange::Updated
    } else {
        FileChange::Created
    })
}

pub(crate) fn remove(path: &Path, block: &ManagedBlock) -> Result<FileChange> {
    let Some(original) = read_utf8_file(path)? else {
        return Ok(FileChange::Absent);
    };

    let rewritten = rewrite(path, &original, block, RewriteMode::Remove)?;
    if !rewritten.found {
        return Ok(FileChange::Absent);
    }

    fs::write(path, rewritten.contents).map_err(|source| Error::io("write file", path, source))?;
    Ok(FileChange::Removed)
}

pub(crate) fn matches(path: &Path, block: &ManagedBlock) -> Result<bool> {
    let Some(contents) = read_utf8_file(path)? else {
        return Ok(false);
    };

    let expected = block.render();
    if contents.contains(expected.trim_end()) {
        return Ok(true);
    }

    let mut cursor = 0;
    while let Some(relative_start) = contents[cursor..].find(&block.start_marker) {
        let start = cursor + relative_start;
        let after_start = start + block.start_marker.len();
        let Some(relative_end) = contents[after_start..].find(&block.end_marker) else {
            return Err(Error::ManagedBlockMissingEnd {
                path: path.to_path_buf(),
                start_marker: block.start_marker.clone(),
                end_marker: block.end_marker.clone(),
            });
        };
        cursor = after_start + relative_end + block.end_marker.len();
    }

    Ok(false)
}

fn read_utf8_file(path: &Path) -> Result<Option<String>> {
    match fs::read(path) {
        Ok(contents) => String::from_utf8(contents)
            .map(Some)
            .map_err(|_| Error::InvalidUtf8File {
                path: path.to_path_buf(),
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::io("read file", path, source)),
    }
}

#[derive(Clone, Copy)]
enum RewriteMode {
    Upsert,
    Remove,
}

struct RewriteResult {
    contents: String,
    found: bool,
}

fn rewrite(
    path: &Path,
    contents: &str,
    block: &ManagedBlock,
    mode: RewriteMode,
) -> Result<RewriteResult> {
    let mut cleaned = String::new();
    let mut cursor = 0;
    let mut found = false;
    let mut inserted = false;

    while let Some(relative_start) = contents[cursor..].find(&block.start_marker) {
        let start = cursor + relative_start;
        cleaned.push_str(&contents[cursor..start]);

        let after_start = start + block.start_marker.len();
        let relative_end = contents[after_start..]
            .find(&block.end_marker)
            .ok_or_else(|| Error::ManagedBlockMissingEnd {
                path: path.to_path_buf(),
                start_marker: block.start_marker.clone(),
                end_marker: block.end_marker.clone(),
            })?;
        let mut end = after_start + relative_end + block.end_marker.len();
        while let Some(ch) = contents[end..].chars().next() {
            if ch == '\n' || ch == '\r' {
                end += ch.len_utf8();
                continue;
            }
            break;
        }
        if matches!(mode, RewriteMode::Upsert) && !inserted {
            cleaned.push_str(&block.render());
            inserted = true;
        }
        cursor = end;
        found = true;
    }

    cleaned.push_str(&contents[cursor..]);
    Ok(RewriteResult {
        contents: match mode {
            RewriteMode::Upsert if found => cleaned,
            _ => cleaned
                .trim_end_matches('\n')
                .trim_end_matches('\r')
                .to_owned(),
        },
        found,
    })
}

fn append_block(existing: &str, block: &ManagedBlock) -> String {
    if existing.trim().is_empty() {
        return block.render();
    }

    format!("{}\n\n{}", existing.trim_end(), block.render())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{ManagedBlock, matches, remove, upsert};
    use crate::model::FileChange;

    #[test]
    fn upsert_is_idempotent() {
        let temp_root = crate::tests::temp_dir("managed-block-upsert");
        let profile = temp_root.join(".shellrc");
        let block = ManagedBlock {
            start_marker: "# >>> shellcomp bash tool >>>".to_owned(),
            end_marker: "# <<< shellcomp bash tool <<<".to_owned(),
            body: "source '/tmp/tool'".to_owned(),
        };

        let first = upsert(&profile, &block).expect("first upsert should succeed");
        let second = upsert(&profile, &block).expect("second upsert should succeed");

        assert_eq!(first, FileChange::Created);
        assert_eq!(second, FileChange::Unchanged);
    }

    #[test]
    fn remove_deletes_all_duplicate_blocks() {
        let temp_root = crate::tests::temp_dir("managed-block-remove");
        let profile = temp_root.join(".shellrc");
        let block = ManagedBlock {
            start_marker: "# >>> shellcomp bash tool >>>".to_owned(),
            end_marker: "# <<< shellcomp bash tool <<<".to_owned(),
            body: "source '/tmp/tool'".to_owned(),
        };

        let duplicate = format!(
            "{}{}\n{}\n{}\n{}\n",
            block.render(),
            "echo keep",
            block.start_marker,
            block.body,
            block.end_marker
        );
        fs::write(&profile, duplicate).expect("profile should be writable");

        let change = remove(&profile, &block).expect("remove should succeed");

        assert_eq!(change, FileChange::Removed);
        let remaining = fs::read_to_string(profile).expect("profile should remain readable");
        assert!(!remaining.contains(&block.start_marker));
        assert!(!remaining.contains(&block.end_marker));
        assert!(remaining.contains("echo keep"));
    }

    #[test]
    fn upsert_replaces_stale_managed_block_body() {
        let temp_root = crate::tests::temp_dir("managed-block-update");
        let profile = temp_root.join(".shellrc");
        let stale = ManagedBlock {
            start_marker: "# >>> shellcomp bash tool >>>".to_owned(),
            end_marker: "# <<< shellcomp bash tool <<<".to_owned(),
            body: "source '/tmp/old-tool'".to_owned(),
        };
        let fresh = ManagedBlock {
            start_marker: stale.start_marker.clone(),
            end_marker: stale.end_marker.clone(),
            body: "source '/tmp/new-tool'".to_owned(),
        };

        upsert(&profile, &stale).expect("stale block should be written");
        let change = upsert(&profile, &fresh).expect("fresh block should be written");

        assert_eq!(change, FileChange::Updated);
        let rendered = fs::read_to_string(profile).expect("profile should remain readable");
        assert!(rendered.contains("/tmp/new-tool"));
        assert!(!rendered.contains("/tmp/old-tool"));
    }

    #[test]
    fn upsert_preserves_existing_block_position() {
        let temp_root = crate::tests::temp_dir("managed-block-position");
        let profile = temp_root.join(".shellrc");
        let stale = ManagedBlock {
            start_marker: "# >>> shellcomp bash tool >>>".to_owned(),
            end_marker: "# <<< shellcomp bash tool <<<".to_owned(),
            body: "source '/tmp/old-tool'".to_owned(),
        };
        let fresh = ManagedBlock {
            start_marker: stale.start_marker.clone(),
            end_marker: stale.end_marker.clone(),
            body: "source '/tmp/new-tool'".to_owned(),
        };
        let contents = format!("export A=1\n{}\necho tail\n", stale.render());
        fs::write(&profile, contents).expect("profile should be writable");

        upsert(&profile, &fresh).expect("upsert should succeed");

        let rendered = fs::read_to_string(profile).expect("profile should remain readable");
        assert!(rendered.starts_with("export A=1\n# >>> shellcomp bash tool >>>"));
        assert!(rendered.contains("echo tail"));
    }

    #[test]
    fn matches_rejects_stale_block_body() {
        let temp_root = crate::tests::temp_dir("managed-block-matches");
        let profile = temp_root.join(".shellrc");
        let stale = ManagedBlock {
            start_marker: "# >>> shellcomp bash tool >>>".to_owned(),
            end_marker: "# <<< shellcomp bash tool <<<".to_owned(),
            body: "source '/tmp/old-tool'".to_owned(),
        };
        let fresh = ManagedBlock {
            start_marker: stale.start_marker.clone(),
            end_marker: stale.end_marker.clone(),
            body: "source '/tmp/new-tool'".to_owned(),
        };

        upsert(&profile, &stale).expect("stale block should be written");

        assert!(!matches(&profile, &fresh).expect("match check should succeed"));
        assert!(matches(&profile, &stale).expect("match check should succeed"));
    }

    #[test]
    fn matches_reports_missing_end_marker() {
        let temp_root = crate::tests::temp_dir("managed-block-matches-corrupt");
        let profile = temp_root.join(".shellrc");
        let block = ManagedBlock {
            start_marker: "# >>> shellcomp bash tool >>>".to_owned(),
            end_marker: "# <<< shellcomp bash tool <<<".to_owned(),
            body: "source '/tmp/tool'".to_owned(),
        };
        fs::write(
            &profile,
            "# >>> shellcomp bash tool >>>\nsource '/tmp/tool'\n",
        )
        .expect("profile should be writable");

        let error = matches(&profile, &block).expect_err("matches should fail");

        assert!(matches!(error, crate::Error::ManagedBlockMissingEnd { .. }));
    }
}
