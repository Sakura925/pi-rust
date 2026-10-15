//! `delegate_task` — 把一个有界子任务委派给子 agent（Claude Code Task /
//! Codex `multi_agent_v1__spawn_agent` 的 rpi 对应物）。
//!
//! 工作目录下的 agent 定义（markdown）即子 agent 的"人格"：
//! - `<cwd>/.ai-flow/agents/{agent}.md`（对齐 .ai-flow 工作区体系，优先）
//! - `<cwd>/.rpi/agents/{agent}.md`（rpi 原生路径）
//!
//! frontmatter 的 `model` 决定子 agent 用的模型（可被参数覆盖），正文作为
//! `--append-system-prompt` 注入嵌套 rpi；`.ai-flow/rules/*.md` 以路径清单
//! 形式附在提示词尾部（子 agent 按需读取，不整包塞入）。
//!
//! 子进程是 `current_exe() -p <task>` —— 始终与父同一构建，天然携带嵌套
//! 追踪环境（`RPI_SESSION_ID` 等父辈 env 直接继承，不走 bash 工具的
//! host-private 剥离路径）。子 agent 的流式输出经 `on_update` 节流回传，
//! TUI 的工具面板因此实时可见"是否真的在跑"。工作目录下的 agent 定义（markdown）即子 agent 的"人格"：

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rpi_agent::agent_tool::AgentTool;
use rpi_agent::error::AgentError;
use rpi_agent::types::{AgentToolResult, TextContentOrImage, ToolResultPartial};
use rpi_ai::types::{Schema, TextContent, Tool};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::env::ExecutionEnv;
use crate::tools::tool_context::ExecutionToolContext;

/// `on_update` 节流窗口：与 bash 工具一致（2s）。
const DELEGATE_UPDATE_THROTTLE_MS: u64 = 2_000;
/// 默认超时（秒）：一个有界子任务不应超过 10 分钟。
const DEFAULT_TIMEOUT_SECONDS: f64 = 600.0;
/// 默认轮数上限（透传给子 rpi 的 `--max-turns`）。
const DEFAULT_MAX_TURNS: u64 = 32;
/// partial 输出保留的尾部行数。
const PARTIAL_TAIL_LINES: usize = 30;

/// Input for the delegate tool.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct DelegateTaskInput {
    /// Agent name — resolves to `.ai-flow/agents/{agent}.md` or
    /// `.rpi/agents/{agent}.md` under the working directory.
    pub agent: String,
    /// The self-contained task brief for the sub-agent.
    pub task: String,
    /// Override the agent definition's frontmatter model.
    #[serde(default)]
    pub model: Option<String>,
    /// Max agent turns for the nested run (default 32).
    #[serde(default)]
    pub max_turns: Option<u64>,
    /// Timeout in seconds (default 600).
    #[serde(default)]
    pub timeout_seconds: Option<f64>,
}

/// Parsed agent definition.
#[derive(Debug, Clone)]
struct AgentDefinition {
    model: Option<String>,
    body: String,
}

/// Parse the minimal frontmatter subset the `.ai-flow/agents` files use:
/// `name` / `description` / `model` / `color` / `injectAgentsMd`. Everything
/// before the closing `---` is metadata; the rest is the prompt body.
fn parse_agent_definition(raw: &str) -> AgentDefinition {
    let mut model = None;
    let body = if let Some(rest) = raw.strip_prefix("---\n") {
        match rest.find("\n---") {
            Some(end) => {
                let frontmatter = &rest[..end];
                for line in frontmatter.lines() {
                    if let Some(value) = line.strip_prefix("model:") {
                        model = Some(value.trim().trim_matches('"').to_string());
                    }
                }
                let after = &rest[end + 4..];
                after.strip_prefix('\n').unwrap_or(after).to_string()
            }
            None => raw.to_string(),
        }
    } else {
        raw.to_string()
    };
    AgentDefinition { model, body }
}

/// Resolve `{agent}.md` under the `.ai-flow/agents` / `.rpi/agents` dirs.
fn resolve_agent_file(cwd: &std::path::Path, agent: &str) -> Option<PathBuf> {
    // Reject path separators: the name must be a plain file stem.
    if agent.contains('/') || agent.contains('\\') || agent.contains("..") {
        return None;
    }
    let stem = agent.trim().trim_end_matches(".md");
    if stem.is_empty() {
        return None;
    }
    for root in [".ai-flow/agents", ".rpi/agents"] {
        let candidate = cwd.join(root).join(format!("{stem}.md"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// List `.ai-flow/rules/*.md` (sorted) so the sub-agent can read what it needs.
fn rules_manifest(cwd: &std::path::Path) -> Vec<String> {
    let dir = cwd.join(".ai-flow/rules");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
        .map(|e| e.path().to_string_lossy().into_owned())
        .collect();
    paths.sort();
    paths
}

struct DelegateTool {
    env: Arc<dyn ExecutionEnv>,
}

/// Shared streaming state between the reader task and the throttled updater.
#[derive(Default)]
struct StreamState {
    lines: Vec<String>,
    last_flush: Option<Instant>,
    /// The sub-agent's CURRENT activity, derived from the json event stream:
    /// "执行 read（path…）" / "思考中…" / streaming text tail. Drives the
    /// delegate panel's live activity line.
    activity: String,
    /// The assistant's final text, accumulated from `message_end` events.
    /// The tool's result when the run completes.
    final_text: String,
}

impl StreamState {
    fn record_event(&mut self, line: &str) {
        self.lines.push(line.to_string());
        // Best-effort JSON event projection: enough of the `RemoteEvent`
        // shape to keep the panel honest about what the sub-agent is doing.
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            return;
        };
        let event_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match event_type {
            "tool_execution_start" => {
                let tool = value
                    .get("toolName")
                    .and_then(|t| t.as_str())
                    .unwrap_or("tool");
                self.activity = format!("执行 {tool}");
            }
            "message_update" => {
                let kind = value
                    .get("eventType")
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                if kind.contains("thinking") {
                    // Never leak reasoning content — presence only.
                    self.activity = "思考中…".to_string();
                } else if let Some(delta) = value.get("delta").and_then(|d| d.as_str()) {
                    if !delta.trim().is_empty() {
                        let tail = delta
                            .lines()
                            .rev()
                            .find(|l| !l.trim().is_empty())
                            .unwrap_or("");
                        self.activity = format!("正在输出：{}", tail.trim());
                    }
                }
            }
            "message_end" => {
                // Assistant text from the message body — the sub-agent's answer.
                let text = value
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter_map(|b| {
                                (b.get("type").and_then(|t| t.as_str()) == Some("text"))
                                    .then(|| b.get("text").and_then(|t| t.as_str()))
                                    .flatten()
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                if !text.trim().is_empty() {
                    self.final_text = text;
                }
                self.activity.clear();
            }
            "turn_start" => {
                self.activity = "新轮次开始".to_string();
            }
            _ => {}
        }
    }
}

#[async_trait]
impl AgentTool for DelegateTool {
    fn schema(&self) -> &Tool {
        static SCHEMA: std::sync::OnceLock<Tool> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| {
            let params = schemars::schema_for!(DelegateTaskInput);
            Tool {
                name: "delegate_task".to_string(),
                description: "Delegate a bounded sub-task to a sub-agent defined in \
                 .ai-flow/agents/ (or .rpi/agents/). The agent definition becomes \
                 the sub-agent's system prompt; the nested run reports streamed \
                 output and returns the sub-agent's final answer. Use for \
                 mechanical, verifiable work (read/search/edit/test/log collection) \
                 per the model-routing rules."
                    .to_string(),
                parameters: Schema::new(
                    serde_json::to_value(params).unwrap_or_default(),
                ),
                constrained_sampling: None,
            }
        })
    }

    fn label(&self) -> &str {
        "delegate_task"
    }

    fn execution_mode(&self) -> rpi_agent::types::ToolExecutionMode {
        // A delegation is a whole side-run; never run two concurrently.
        rpi_agent::types::ToolExecutionMode::Sequential
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        signal: CancellationToken,
        on_update: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        let _ = tool_call_id;
        let input: DelegateTaskInput = serde_json::from_value(params).map_err(|e| {
            AgentError::Tool(format!("delegate_task: invalid arguments: {e}"))
        })?;

        let cwd = self.env.cwd().to_path_buf();
        let agent_path =
            resolve_agent_file(&cwd, &input.agent).ok_or_else(|| {
                AgentError::Tool(format!(
                    "delegate_task: unknown agent {:?} — no .ai-flow/agents/{0}.md or \
                     .rpi/agents/{0}.md under {}",
                    input.agent,
                    cwd.display()
                ))
            })?;
        let raw = std::fs::read_to_string(&agent_path).map_err(|e| {
            AgentError::Tool(format!("delegate_task: read {agent_path:?}: {e}"))
        })?;
        let definition = parse_agent_definition(&raw);

        let model = input.model.clone().or(definition.model);
        let timeout_seconds =
            input.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS).max(1.0);
        let max_turns = input.max_turns.unwrap_or(DEFAULT_MAX_TURNS);

        // System prompt: the agent definition body + the rules manifest (the
        // sub-agent reads the rule files it needs itself — no bulk injection).
        let rules = rules_manifest(&cwd);
        let rules_note = if rules.is_empty() {
            String::new()
        } else {
            format!(
                "\n\n---\nProject rules live at (read the ones relevant to the task):\n{}\n",
                rules.join("\n")
            )
        };
        let system_prompt = format!(
            "You are the sub-agent defined below. Complete the delegated task, then stop.\n\n{}{}",
            definition.body, rules_note
        );

        // Always run the same rpi binary as the parent.
        let exe = std::env::current_exe()
            .map_err(|e| AgentError::Tool(format!("delegate_task: current_exe: {e}")))?;

        let mut command = tokio::process::Command::new(&exe);
        command
            .arg("-p")
            .arg(&input.task)
            .arg("--mode")
            .arg("json")
            .arg("--append-system-prompt")
            .arg(&system_prompt)
            .current_dir(&cwd)
            // Turn budget rides the same env var the harness reads
            // (`RPI_MAX_TURNS_PER_RUN`); there is no CLI flag for it.
            .env("RPI_MAX_TURNS_PER_RUN", max_turns.to_string());
        // P1 review fix: own the child's lifetime — if this future is dropped
        // before completion (loop cancellation), the child is killed instead of
        // being orphaned.
        command.kill_on_drop(true);
        // P1 review fix: new process group so a timeout/cancel terminates the
        // WHOLE tree (nested rpi spawns shells/builds of its own).
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            // New process group = pgid equals the child pid; termination
            // targets the group (see kill_process).
            let _ = command.as_std_mut().process_group(0);
        }
        if let Some(model) = model.as_deref() {
            command.arg("--model").arg(model);
        }

        let started = Instant::now();
        let mut child = command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                AgentError::Tool(format!("delegate_task: spawn {:?}: {e}", exe))
            })?;
        let child_id = child.id();

        let stdout = child
            .stdout
            .take()
            .expect("delegate_task: stdout piped above");
        let stderr = child
            .stderr
            .take()
            .expect("delegate_task: stderr piped above");
        let state = Arc::new(Mutex::new(StreamState::default()));
        let reader_state = Arc::clone(&state);

        // P1 review fix: stderr must be drained concurrently or a chatty child
        // (plugin logs, diagnostics) fills the 64 KiB pipe buffer and blocks
        // forever, riding the whole timeout window. Keep a bounded tail and
        // surface it on failure.
        let stderr_state = Arc::new(Mutex::new(Vec::<String>::new()));
        let stderr_tail_reader = Arc::clone(&stderr_state);
        let stderr_reader = tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut tail = stderr_tail_reader.lock().unwrap();
                tail.push(line);
                let overflow = tail.len().saturating_sub(PARTIAL_TAIL_LINES);
                if overflow > 0 {
                    tail.drain(..overflow);
                }
            }
        });

        // Reader task: accumulate lines; the throttled flush runs inline on each
        // line read (cheap: just a time check + tail clone).
        let update_counter: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
        let reader = tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                let mut guard = reader_state.lock().unwrap();
                guard.record_event(&line);
                let should_flush = guard
                    .last_flush
                    .is_none_or(|t| t.elapsed() >= Duration::from_millis(DELEGATE_UPDATE_THROTTLE_MS));
                if should_flush {
                    guard.last_flush = Some(Instant::now());
                    // The partial payload is the CURRENT ACTIVITY — what the
                    // sub-agent is doing right now — not a dump of raw stdout:
                    // `-p --mode json` only emits events, and the panel's live
                    // line exists precisely to show "执行 read / 思考中…".
                    let activity = guard.activity.clone();
                    drop(guard);
                    if !activity.is_empty() {
                        update_counter.fetch_add(1, Ordering::Relaxed);
                        on_update(AgentToolResult {
                            content: vec![TextContentOrImage::Text(TextContent {
                                kind: Default::default(),
                                text: activity,
                                text_signature: None,
                            })],
                            details: serde_json::json!({"partial": true}),
                            usage: None,
                            added_tool_names: Vec::new(),
                            terminate: false,
                        });
                    }
                }
            }
        });

        // Race: completion, timeout, cancellation.
        let timeout = Duration::from_secs_f64(timeout_seconds);
        let exit_status = loop {
            tokio::select! {
                status = child.wait() => break Some(status.map_err(|e| {
                    AgentError::Tool(format!("delegate_task: wait: {e}"))
                })?),
                _ = tokio::time::sleep(timeout) => {
                    if let Some(pid) = child_id {
                        let _ = kill_process(pid);
                    }
                    let _ = child.wait().await;
                    break None;
                }
                _ = signal.cancelled() => {
                    if let Some(pid) = child_id {
                        let _ = kill_process(pid);
                    }
                    let _ = child.wait().await;
                    break None;
                }
            }
        };

        // Give the readers a moment to drain the pipes after process exit.
        let _ = tokio::time::timeout(Duration::from_secs(2), reader).await;
        let _ = tokio::time::timeout(Duration::from_secs(2), stderr_reader).await;
        // Prefer the sub-agent's final answer (extracted from `message_end`
        // events) over the raw event-line dump; fall back to the raw lines
        // when no text block ever landed (e.g. an early crash).
        let final_output = {
            let guard = state.lock().unwrap();
            if guard.final_text.trim().is_empty() {
                guard.lines.join("\n")
            } else {
                guard.final_text.clone()
            }
        };
        let stderr_tail = {
            let tail = stderr_state.lock().unwrap();
            tail.join("\n")
        };
        let elapsed = started.elapsed();

        let (status_label, exit_code) = match exit_status {
            Some(status) => (Some(status.to_string()), status.code()),
            None => (None, None),
        };

        // Timeout / cancel diagnostics take precedence in the message so the
        // parent agent can decide to retry (per luna protocol: one retry max).
        let outcome = if exit_status.is_none() {
            if signal.is_cancelled() {
                "cancelled".to_string()
            } else {
                "timeout".to_string()
            }
        } else if exit_code.unwrap_or(0) == 0 {
            "completed".to_string()
        } else {
            format!("failed (exit {exit_code:?})")
        };

        let mut content = String::new();
        if outcome != "completed" {
            content.push_str(&format!(
                "Sub-agent {outcome} after {:.1}s. Output so far (tail):\n\n",
                elapsed.as_secs_f64()
            ));
        }
        content.push_str(&final_output);
        // P1 review fix: surface the child's stderr tail so failures diagnose
        // themselves (the pipe was previously never read at all).
        if !stderr_tail.trim().is_empty() {
            content.push_str("\n\n[stderr tail]\n");
            content.push_str(&stderr_tail);
        }

        Ok(AgentToolResult {
            content: vec![TextContentOrImage::Text(TextContent {
                kind: Default::default(),
                text: content,
                text_signature: None,
            })],
            details: serde_json::json!({
                "agent": input.agent,
                "model": model,
                "outcome": outcome,
                "exit_status": status_label,
                "elapsed_ms": elapsed.as_millis() as u64,
                "max_turns": max_turns,
                "timeout_seconds": timeout_seconds,
            }),
            usage: None,
            added_tool_names: Vec::new(),
            terminate: false,
        })
    }
}

/// Kill the child's whole process group — the spawn side puts the nested rpi
/// in its own group (pgid == pid), and its shells/builds live in that group,
/// so a group kill terminates the entire tree instead of leaving orphaned
/// descendants running (P1 review finding: cancel/timeout previously killed
/// only the direct pid).
fn kill_process(pid: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .arg("-9")
            .arg("--")
            .arg(format!("-{pid}"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|_| ())
    }
    #[cfg(windows)]
    {
        std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|_| ())
    }
}

/// Options for `create_delegate_task_tool` (currently context-only; kept for
/// future knobs like a default agent).
#[derive(Clone, Default)]
pub struct DelegateToolOptions;

/// Build the `delegate_task` tool bound to an execution context. Mirrors the
/// other tool factories: takes `&ExecutionToolContext`, keeps only the env
/// slice a delegation needs.
pub fn create_delegate_task_tool(context: &ExecutionToolContext) -> Arc<dyn AgentTool> {
    Arc::new(DelegateTool {
        env: Arc::clone(context.env()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_body() {
        let raw = "---\nname: \"luna\"\nmodel: \"gpt-5.6-luna\"\ninjectAgentsMd: true\n---\n\n# Luna\n\nDo the thing.";
        let def = parse_agent_definition(raw);
        assert_eq!(def.model.as_deref(), Some("gpt-5.6-luna"));
        assert!(def.body.contains("# Luna"));
        assert!(!def.body.contains("model:"));
    }

    #[test]
    fn frontmatter_without_model_yields_none() {
        let def = parse_agent_definition("just a body");
        assert!(def.model.is_none());
        assert_eq!(def.body, "just a body");
    }

    #[test]
    fn rejects_traversal_names() {
        let tmp = std::env::temp_dir();
        assert!(resolve_agent_file(&tmp, "../../etc/passwd").is_none());
        assert!(resolve_agent_file(&tmp, "a/b").is_none());
        assert!(resolve_agent_file(&tmp, "").is_none());
    }

    #[test]
    fn resolves_agent_md_from_ai_flow_first() {
        let base = std::env::temp_dir().join("rpi-delegate-test");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join(".ai-flow/agents")).unwrap();
        std::fs::create_dir_all(base.join(".rpi/agents")).unwrap();
        std::fs::write(
            base.join(".ai-flow/agents").join("alpha.md"),
            "---\nmodel: \"m1\"\n---\nai-flow body",
        )
        .unwrap();
        std::fs::write(
            base.join(".rpi/agents").join("beta.md"),
            "---\nmodel: \"m2\"\n---\nrpi body",
        )
        .unwrap();

        let found = resolve_agent_file(&base, "alpha").unwrap();
        assert!(found.ends_with(".ai-flow/agents/alpha.md"));
        let found = resolve_agent_file(&base, "beta.md").unwrap();
        assert!(found.ends_with(".rpi/agents/beta.md"));
        assert!(resolve_agent_file(&base, "gamma").is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rules_manifest_lists_sorted_md_files() {
        let base = std::env::temp_dir().join("rpi-delegate-rules");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join(".ai-flow/rules")).unwrap();
        std::fs::write(base.join(".ai-flow/rules").join("b.md"), "x").unwrap();
        std::fs::write(base.join(".ai-flow/rules").join("a.md"), "x").unwrap();
        std::fs::write(base.join(".ai-flow/rules").join("notes.txt"), "x").unwrap();
        let manifest = rules_manifest(&base);
        let names: Vec<&str> = manifest
            .iter()
            .map(|p| std::path::Path::new(p).file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, vec!["a.md", "b.md"]);
        let _ = std::fs::remove_dir_all(&base);
    }
}
