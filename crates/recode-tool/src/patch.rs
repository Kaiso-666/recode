use crate::{Tool, ToolContext};
use async_trait::async_trait;
use recode_schema::permission::Decision;
use recode_schema::tool::{ToolFailure, ToolOutcome, ToolResult};
use serde::Deserialize;
use serde_json::{json, Value};

pub struct PatchTool;

#[derive(Deserialize)]
struct Input {
    /// Unified diff, optionally with `---`/`+++` file headers for multiple files.
    patch: String,
}

#[async_trait]
impl Tool for PatchTool {
    fn name(&self) -> &str {
        "patch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff to one or more files. Standard `---`/`+++` headers and \
         `@@` hunks are supported; `a/` and `b/` prefixes are stripped. Every hunk must \
         match exactly, otherwise nothing is written. Use edit for a single small change."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "Unified diff text. Multiple file sections are allowed."
                }
            },
            "required": ["patch"],
            "additionalProperties": false
        })
    }

    fn permission(&self) -> &str {
        "edit"
    }

    fn default_decision(&self) -> Decision {
        Decision::Ask
    }

    fn title(&self, _input: &Value) -> String {
        "Apply patch".to_owned()
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> ToolResult {
        let input: Input = serde_json::from_value(input)
            .map_err(|error| ToolFailure::new(format!("invalid patch arguments: {error}")))?;
        let files = parse_patch(&input.patch)?;
        if files.is_empty() {
            return Err(ToolFailure::new("the patch contains no file sections"));
        }
        let mut report = String::new();
        let mut changed = Vec::new();
        for file in &files {
            let path = context.resolve_path(&file.path)?;
            changed.push(path.clone());
            let existing = tokio::fs::read_to_string(&path).await.ok();
            let (updated, removed) = apply_file(file, existing.as_deref())?;
            match updated {
                Some(updated) => {
                    write_patch(&path, &updated).await?;
                    report.push_str(&format!(
                        "{} {} (+{} -{})\n",
                        if existing.is_some() {
                            "modified"
                        } else {
                            "created"
                        },
                        file.path,
                        file.added(),
                        file.removed()
                    ));
                }
                None => {
                    if removed {
                        tokio::fs::remove_file(&path).await.map_err(|error| {
                            ToolFailure::new(format!("cannot delete {}: {error}", path.display()))
                        })?;
                        report.push_str(&format!("deleted {}\n", file.path));
                    }
                }
            }
        }
        Ok(ToolOutcome {
            title: Some(format!("Patch {} file(s)", files.len())),
            output: report,
            metadata: Some(json!({"files": changed, "fileCount": files.len()})),
            attachments: Vec::new(),
        })
    }
}

struct FilePatch {
    path: String,
    hunks: Vec<Hunk>,
    deleted: bool,
    created: bool,
}

struct Hunk {
    old_lines: Vec<String>,
    new_lines: Vec<String>,
}

impl FilePatch {
    fn added(&self) -> usize {
        self.hunks.iter().map(|hunk| hunk.new_lines.len()).sum()
    }

    fn removed(&self) -> usize {
        self.hunks.iter().map(|hunk| hunk.old_lines.len()).sum()
    }
}

fn parse_patch(text: &str) -> Result<Vec<FilePatch>, ToolFailure> {
    let lines: Vec<&str> = text.lines().collect();
    let mut files = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if !lines[index].starts_with("--- ") {
            index += 1;
            continue;
        }
        let old_path = strip_header_path(&lines[index][4..]);
        index += 1;
        if index >= lines.len() || !lines[index].starts_with("+++ ") {
            return Err(ToolFailure::new("patch header is missing its `+++` line"));
        }
        let new_path = strip_header_path(&lines[index][4..]);
        index += 1;
        let created = old_path == "/dev/null" || old_path.is_empty();
        let deleted = new_path == "/dev/null" || new_path.is_empty();
        let path = if deleted { old_path } else { new_path };
        let path = path
            .strip_prefix("a/")
            .or_else(|| path.strip_prefix("b/"))
            .unwrap_or(&path)
            .to_owned();
        let mut hunks = Vec::new();
        while index < lines.len() && lines[index].starts_with("@@") {
            let header = lines[index];
            let (old_count, new_count) = parse_hunk_header(header)?;
            index += 1;
            let mut old_lines = Vec::new();
            let mut new_lines = Vec::new();
            while index < lines.len() {
                let line = lines[index];
                if line.starts_with("@@") || line.starts_with("--- ") {
                    break;
                }
                if line.starts_with("diff --git") {
                    break;
                }
                if line == "\\ No newline at end of file" {
                    index += 1;
                    continue;
                }
                let mut chars = line.chars();
                match chars.next() {
                    Some(' ') => {
                        old_lines.push(chars.as_str().to_owned());
                        new_lines.push(chars.as_str().to_owned());
                    }
                    Some('-') => old_lines.push(chars.as_str().to_owned()),
                    Some('+') => new_lines.push(chars.as_str().to_owned()),
                    None => {
                        old_lines.push(String::new());
                        new_lines.push(String::new());
                    }
                    Some(_) => break,
                }
                index += 1;
            }
            if old_count.is_some_and(|count| count != old_lines.len())
                || new_count.is_some_and(|count| count != new_lines.len())
            {
                return Err(ToolFailure::new(format!(
                    "hunk {header:?} expected {} old / {} new lines, found {} / {}",
                    old_count.unwrap_or(0),
                    new_count.unwrap_or(0),
                    old_lines.len(),
                    new_lines.len()
                )));
            }
            hunks.push(Hunk {
                old_lines,
                new_lines,
            });
        }
        if hunks.is_empty() {
            return Err(ToolFailure::new(format!(
                "no hunks found for {path}; is this a valid unified diff?"
            )));
        }
        files.push(FilePatch {
            path,
            hunks,
            deleted,
            created,
        });
    }
    Ok(files)
}

fn strip_header_path(header: &str) -> String {
    header
        .split('\t')
        .next()
        .unwrap_or(header)
        .trim()
        .to_owned()
}

fn parse_hunk_header(header: &str) -> Result<(Option<usize>, Option<usize>), ToolFailure> {
    let rest = header
        .strip_prefix("@@")
        .and_then(|rest| rest.split("@@").next())
        .ok_or_else(|| ToolFailure::new(format!("invalid hunk header {header:?}")))?
        .trim();
    let mut parts = rest.split(' ');
    let old = parts.next().unwrap_or_default();
    let new = parts.next().unwrap_or_default();
    Ok((parse_range(old), parse_range(new)))
}

fn parse_range(value: &str) -> Option<usize> {
    let value = value.trim_start_matches(['-', '+']);
    match value.split_once(',') {
        Some((_, count)) => count.parse().ok(),
        None => Some(1),
    }
}

fn apply_file(
    file: &FilePatch,
    source: Option<&str>,
) -> Result<(Option<String>, bool), ToolFailure> {
    let original = source.unwrap_or("");
    let eol = if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let trailing_newline = original.ends_with('\n') || original.is_empty();
    let mut lines: Vec<String> = if original.is_empty() {
        Vec::new()
    } else {
        original
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
            .collect()
    };
    if trailing_newline && lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let mut search_from = 0usize;
    for hunk in &file.hunks {
        let position = find_hunk(&lines, &hunk.old_lines, search_from).ok_or_else(|| {
            ToolFailure::new(format!(
                "hunk did not match {}:\n{}",
                file.path,
                hunk.old_lines.join("\n")
            ))
        })?;
        lines.splice(
            position..position + hunk.old_lines.len(),
            hunk.new_lines.iter().cloned(),
        );
        search_from = position + hunk.new_lines.len();
    }
    if file.deleted {
        return Ok((None, true));
    }
    let mut updated = lines.join(eol);
    if trailing_newline && !updated.is_empty() {
        updated.push_str(eol);
    }
    if file.created && source.is_some() {
        return Err(ToolFailure::new(format!(
            "{} already exists but the patch creates it",
            file.path
        )));
    }
    Ok((Some(updated), false))
}

fn find_hunk(lines: &[String], old: &[String], from: usize) -> Option<usize> {
    if old.is_empty() {
        return Some(from.min(lines.len()));
    }
    if old.len() > lines.len() {
        return None;
    }
    (from..=lines.len() - old.len()).find(|start| {
        lines[*start..*start + old.len()]
            .iter()
            .zip(old)
            .all(|(left, right)| left == right)
    })
}

async fn write_patch(path: &std::path::Path, content: &str) -> Result<(), ToolFailure> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| {
            ToolFailure::new(format!("cannot create {}: {error}", parent.display()))
        })?;
    }
    tokio::fs::write(path, content)
        .await
        .map_err(|error| ToolFailure::new(format!("cannot write {}: {error}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str =
        "--- a/hello.txt\n+++ b/hello.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n";

    #[test]
    fn parses_and_applies_a_hunk() {
        let files = parse_patch(DIFF).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "hello.txt");
        let (updated, _) = apply_file(&files[0], Some("one\ntwo\nthree\n")).unwrap();
        assert_eq!(updated.unwrap(), "one\nTWO\nthree\n");
    }

    #[test]
    fn applies_multiple_files_and_preserves_crlf() {
        let diff = "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n--- a/b.txt\n+++ b/b.txt\n@@ -1 +1,2 @@\n-x\n+y\n+z\n";
        let files = parse_patch(diff).unwrap();
        assert_eq!(files.len(), 2);
        let (a, _) = apply_file(&files[0], Some("old\r\n")).unwrap();
        assert_eq!(a.unwrap(), "new\r\n");
        let (b, _) = apply_file(&files[1], Some("x\n")).unwrap();
        assert_eq!(b.unwrap(), "y\nz\n");
    }

    #[test]
    fn rejects_a_mismatched_hunk() {
        let files = parse_patch(DIFF).unwrap();
        let error = apply_file(&files[0], Some("something else\n")).unwrap_err();
        assert!(error.message.contains("did not match"));
    }

    #[tokio::test]
    async fn patch_tool_writes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "one\ntwo\nthree\n").unwrap();
        let registry = crate::ToolRegistry::builtin();
        let context = ToolContext::new(
            recode_schema::id::SessionId::new(),
            recode_schema::id::MessageId::new(),
            "call",
            dir.path().to_path_buf(),
        );
        registry
            .execute("patch", json!({"patch": DIFF}), &context)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
            "one\nTWO\nthree\n"
        );
    }
}
