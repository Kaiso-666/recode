use crate::id::SessionId;
use crate::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Ask,
    Deny,
}

impl Decision {
    pub fn is_denied(self) -> bool {
        matches!(self, Decision::Deny)
    }

    pub fn requires_approval(self) -> bool {
        matches!(self, Decision::Ask)
    }
}

/// One compiled permission rule. `permission` matches the tool/permission name
/// (for example `bash`, `edit`, `external_directory`); `pattern` matches the
/// specific resource (a command, a path, a URL).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub permission: String,
    pub pattern: String,
    pub action: Decision,
}

impl Rule {
    pub fn new(
        permission: impl Into<String>,
        pattern: impl Into<String>,
        action: Decision,
    ) -> Self {
        Self {
            permission: permission.into(),
            pattern: pattern.into(),
            action,
        }
    }

    pub fn allow(permission: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self::new(permission, pattern, Decision::Allow)
    }

    pub fn ask(permission: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self::new(permission, pattern, Decision::Ask)
    }

    pub fn deny(permission: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self::new(permission, pattern, Decision::Deny)
    }
}

/// Ordered rules; the last match wins, mirroring opencode's `findLast`
/// semantics so project rules can override global ones.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Ruleset {
    pub rules: Vec<Rule>,
}

impl Ruleset {
    pub fn new(rules: Vec<Rule>) -> Self {
        Self { rules }
    }

    pub fn push(&mut self, rule: Rule) {
        self.rules.push(rule);
    }

    pub fn extend(&mut self, other: Ruleset) {
        self.rules.extend(other.rules);
    }

    /// Evaluate a permission/resource pair. Returns `None` when no rule
    /// matches, letting the caller apply the tool's default.
    pub fn evaluate(&self, permission: &str, pattern: &str) -> Option<Decision> {
        self.rules
            .iter()
            .rev()
            .find(|rule| {
                wildcard_match(&rule.permission, permission)
                    && wildcard_match(&rule.pattern, pattern)
            })
            .map(|rule| rule.action)
    }

    pub fn is_wholly_denied(&self, permission: &str) -> bool {
        matches!(self.evaluate(permission, "*"), Some(Decision::Deny))
    }
}

/// Glob-ish matching: `*` matches any run of characters (including `/`), `?`
/// matches one character. Permission patterns are intentionally simple.
pub fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    // Iterative backtracking glob; O(n*m) worst case, no allocation.
    let (mut p, mut v) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while v < value.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == value[v]) {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = v;
            p += 1;
        } else if let Some(star_pos) = star {
            p = star_pos + 1;
            mark += 1;
            v = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    pub id: String,
    pub session_id: SessionId,
    pub permission: String,
    #[serde(default)]
    pub patterns: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub metadata: serde_json::Value,
    pub created_at: Timestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionReply {
    Once,
    Always,
    Reject,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_matching_rule_wins() {
        let rules = Ruleset::new(vec![
            Rule::allow("bash", "*"),
            Rule::ask("bash", "git push*"),
            Rule::deny("edit", "*.env"),
        ]);
        assert_eq!(rules.evaluate("bash", "ls"), Some(Decision::Allow));
        assert_eq!(
            rules.evaluate("bash", "git push origin"),
            Some(Decision::Ask)
        );
        assert_eq!(rules.evaluate("edit", ".env"), Some(Decision::Deny));
        assert_eq!(rules.evaluate("edit", "src/main.rs"), None);
    }

    #[test]
    fn wildcards_match_paths_and_commands() {
        assert!(wildcard_match("*", "anything/at all"));
        assert!(wildcard_match("git *", "git status"));
        assert!(!wildcard_match("git *", "npm install"));
        assert!(wildcard_match("*.rs", "src/main.rs"));
        assert!(wildcard_match("a?c", "abc"));
        assert!(!wildcard_match("a?c", "ac"));
    }

    #[test]
    fn wholly_denied_detects_star_deny() {
        let rules = Ruleset::new(vec![Rule::deny("edit", "*")]);
        assert!(rules.is_wholly_denied("edit"));
        assert!(!rules.is_wholly_denied("bash"));
    }
}
