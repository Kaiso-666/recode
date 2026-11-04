use crate::{Tool, ToolContext};
use async_trait::async_trait;
use recode_schema::permission::Decision;
use recode_schema::tool::{ToolFailure, ToolOutcome, ToolResult};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const STREAM_LIMIT: usize = 160_000;

pub struct BashTool;

#[derive(Deserialize)]
struct Input {
    command: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    workdir: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a shell command in the project directory and return its combined output. \
         Use this for builds, tests, git, package managers, and any command-line tool. \
         On Windows the command runs in PowerShell; on Unix it runs in bash. Prefer \
         dedicated tools (read, write, edit, grep, glob) for file operations. The \
         working directory defaults to the project root; paths outside the project \
         are rejected unless permissions allow them."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute."
                },
                "description": {
                    "type": "string",
                    "description": "Short description of what the command does, shown to the user."
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default 120000, max 600000).",
                    "minimum": 1
                },
                "workdir": {
                    "type": "string",
                    "description": "Working directory relative to the project root."
                }
            },
            "required": ["command"],
            "additionalProperties": false
        })
    }

    fn permission(&self) -> &str {
        "bash"
    }

    fn default_decision(&self) -> Decision {
        Decision::Ask
    }

    fn title(&self, input: &Value) -> String {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("command");
        let first_line = command.lines().next().unwrap_or(command);
        format!("$ {}", crate::truncate_middle(first_line, 90))
    }

    fn permission_patterns(&self, input: &Value) -> Vec<String> {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if command.is_empty() {
            Vec::new()
        } else {
            vec![command.to_owned()]
        }
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> ToolResult {
        let input: Input = serde_json::from_value(input)
            .map_err(|error| ToolFailure::new(format!("invalid bash arguments: {error}")))?;
        if input.command.trim().is_empty() {
            return Err(ToolFailure::new("command must not be empty"));
        }
        let directory = match &input.workdir {
            Some(workdir) => context.resolve_path(workdir)?,
            None => context.directory.clone(),
        };
        if !directory.is_dir() {
            return Err(ToolFailure::new(format!(
                "working directory does not exist: {}",
                directory.display()
            )));
        }
        let timeout = Duration::from_millis(
            input
                .timeout_ms
                .unwrap_or(DEFAULT_TIMEOUT_MS)
                .clamp(1, MAX_TIMEOUT_MS),
        );

        let mut command = shell_command(&input.command);
        command
            .current_dir(&directory)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        scrub_environment(&mut command);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.as_std_mut().creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }

        let mut child = command
            .spawn()
            .map_err(|error| ToolFailure::new(format!("failed to start command: {error}")))?;
        let pid = child.id().unwrap_or(0);
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ToolFailure::new("failed to capture stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ToolFailure::new("failed to capture stderr"))?;
        let out_task = tokio::spawn(read_capped(stdout, STREAM_LIMIT));
        let err_task = tokio::spawn(read_capped(stderr, STREAM_LIMIT));

        let mut timed_out = false;
        let status = tokio::select! {
            _ = context.abort.cancelled() => {
                kill_tree(pid).await;
                None
            }
            _ = tokio::time::sleep(timeout) => {
                timed_out = true;
                kill_tree(pid).await;
                None
            }
            status = child.wait() => Some(status),
        };
        let status = match status {
            Some(status) => status.map_err(|error| {
                ToolFailure::new(format!("failed to wait for command: {error}"))
            })?,
            None => child
                .wait()
                .await
                .map_err(|error| ToolFailure::new(format!("failed to reap command: {error}")))?,
        };
        let (stdout, stdout_truncated) = out_task
            .await
            .map_err(|error| ToolFailure::new(format!("stdout reader failed: {error}")))?;
        let (stderr, stderr_truncated) = err_task
            .await
            .map_err(|error| ToolFailure::new(format!("stderr reader failed: {error}")))?;

        let mut output = String::new();
        if !stdout.is_empty() {
            output.push_str(&stdout);
        }
        if !stderr.is_empty() {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&stderr);
        }
        let exit_code = status.code();
        if timed_out {
            output.push_str(&format!(
                "\n[command timed out after {} ms and was terminated]",
                timeout.as_millis()
            ));
        } else if let Some(code) = exit_code {
            output.push_str(&format!("\n[exit code: {code}]"));
        }
        if output.trim().is_empty() {
            output = "(no output)".to_owned();
        }
        let title = input
            .description
            .clone()
            .unwrap_or_else(|| self.title(&json!({"command": input.command})));
        Ok(ToolOutcome {
            title: Some(title),
            output,
            metadata: Some(json!({
                "exitCode": exit_code,
                "timedOut": timed_out,
                "truncated": stdout_truncated || stderr_truncated
            })),
            attachments: Vec::new(),
        })
    }
}

fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("powershell.exe");
        cmd.arg("-NoLogo")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-Command")
            .arg(command);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("bash");
        cmd.arg("-lc").arg(command);
        cmd
    }
}

/// Remove credentials from the child environment. Commands that genuinely
/// need a secret can be granted one explicitly by the user.
fn scrub_environment(command: &mut Command) {
    let secrets = [
        "KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "AUTH",
    ];
    let keep = [
        "PATH",
        "PATHEXT",
        "HOME",
        "USERPROFILE",
        "SYSTEMROOT",
        "WINDIR",
        "TEMP",
        "TMP",
        "SHELL",
        "LANG",
        "LC_ALL",
        "TERM",
        "PWD",
        "COMSPEC",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMFILES",
        "PROGRAMDATA",
        "NUMBER_OF_PROCESSORS",
        "PROCESSOR_ARCHITECTURE",
    ];
    for (name, _) in std::env::vars() {
        let upper = name.to_ascii_uppercase();
        let is_secret = secrets.iter().any(|needle| upper.contains(needle));
        let is_kept = keep.contains(&upper.as_str());
        if is_secret && !is_kept {
            command.env_remove(&name);
        }
    }
}

async fn kill_tree(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill")
            .args(["-9", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }
}

async fn read_capped<R>(mut reader: R, limit: usize) -> (String, bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut collected = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut truncated = false;
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => {
                if collected.len() < limit {
                    let remaining = limit - collected.len();
                    collected.extend_from_slice(&buffer[..read.min(remaining)]);
                    if read > remaining {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
            }
            Err(_) => break,
        }
    }
    (String::from_utf8_lossy(&collected).to_string(), truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolRegistry;
    use recode_schema::id::{MessageId, SessionId};

    fn context(dir: std::path::PathBuf) -> ToolContext {
        ToolContext::new(SessionId::new(), MessageId::new(), "call", dir)
    }

    #[tokio::test]
    async fn runs_echo_in_project_directory() {
        let dir = tempfile::tempdir().unwrap();
        let registry = ToolRegistry::builtin();
        let outcome = registry
            .execute(
                "bash",
                json!({"command": "echo hello"}),
                &context(dir.path().to_path_buf()),
            )
            .await
            .unwrap();
        assert!(outcome.output.contains("hello"));
        assert_eq!(outcome.metadata.unwrap()["exitCode"], 0);
    }

    #[tokio::test]
    async fn reports_nonzero_exit() {
        let dir = tempfile::tempdir().unwrap();
        let registry = ToolRegistry::builtin();
        let outcome = registry
            .execute(
                "bash",
                json!({"command": "exit 3"}),
                &context(dir.path().to_path_buf()),
            )
            .await
            .unwrap();
        assert!(outcome.output.contains("exit code: 3"));
    }
}
