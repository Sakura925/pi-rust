//! Working-state reminders: periodic status-check injections and repeat-call
//! notices, both delivered through the `after_tool_results` hook.
//!
//! # Why this exists
//!
//! The forensics doc (`docs/debugging/llm-repetition-forensics.md` §八) showed a
//! run where the model re-derived its plan every turn: the plan lived only in
//! `thinking`, which the openai-completions path drops, so nothing carried the
//! working state forward. The system prompt now keeps state in the visible
//! channel (§9.1), but that relies on the model's compliance — a long run can
//! still drift.
//!
//! This module makes the harness *ask* for the state at bounded moments
//! instead of trusting it:
//!
//! - **Status check** (every `interval` tool-bearing turns): a one-line
//!   "goal / current step / next action" reminder, so the visible channel gets
//!   refreshed mid-run without asking for a full plan restatement every turn.
//! - **Repeat notice** (the same tool calls with identical arguments N turns
//!   in a row): the forensics data showed exact repeats are rare (3/148) but
//!   when they *do* run in a streak it is the clearest available stuck signal —
//!   one nudge to change approach, never a stop.
//!
//! Both are **reminders, not brakes**: they inject a short user message into
//! the loop context and the run continues. The turn ceiling
//! (`crate::run_budget`) remains the only hard stop.
//!
//! # Injection semantics
//!
//! The reminder message lives in the loop context only — it is NOT part of
//! `new_messages`, so it is never persisted. That is intentional: the reminder
//! is the prompt, the model's *reply* (assistant text, which is persisted) is
//! the durable state. A stale reminder must not pollute the next run's
//! context.
//!
//! # Off by default (upstream parity)
//!
//! Like `RunBudget`, neither reminder is enabled unless the operator asks:
//!
//! ```text
//! RPI_WORKING_STATE_INTERVAL=10 rpi   # status check every 10 tool turns
//! RPI_REPEAT_TOOL_REMINDER=1 rpi      # notice after 3 identical call turns
//! ```

use std::sync::{Arc, Mutex};

use rpi_agent::hooks::AfterToolResults;
use rpi_agent::message::AgentMessage;
use rpi_agent::types::{AgentLoopTurnUpdate, ShouldStopAfterTurnContext};
use rpi_ai::types::{AssistantMessage, Content, UserContent, UserMessage};

/// Environment variable holding the status-check interval (tool-bearing turns).
/// Unset/empty/non-numeric/`0` disables the status check.
pub const WORKING_STATE_INTERVAL_ENV: &str = "RPI_WORKING_STATE_INTERVAL";

/// Environment variable switching the repeat-call notice on (`1`/`true`/`on`).
pub const REPEAT_TOOL_REMINDER_ENV: &str = "RPI_REPEAT_TOOL_REMINDER";

/// How many consecutive identical tool-call turns trigger the repeat notice.
pub const REPEAT_TRIGGER_TURNS: u32 = 3;

/// Timestamps of reminder messages injected THIS run. Reminders are transient
/// by design (they live in the loop context only — see the module docs), so
/// any path that could persist context messages must exclude them. Chief
/// offender: mid-run compaction, which turns `context.messages` into entries
/// and can carry a reminder into the retained tail.
///
/// Identity is the FULL injected message, not a text prefix and not a bare
/// timestamp: a user may legitimately send a message starting with
/// `[repeat notice]` (e.g. asking what the notice means), and a prefix check
/// would silently drop it from compaction; conversely a real user message
/// that happens to share the injected message's millisecond would be dropped
/// by a timestamp-only check. Only messages this harness actually injected —
/// registered by full equality (content + timestamp) at injection time — are
/// transient. The registry is per-run and stays empty when the reminders are
/// disabled, so the compaction filter costs and risks nothing by default.
pub type ReminderRegistry = Arc<Mutex<Vec<UserMessage>>>;

/// Create an empty per-run reminder registry.
pub fn reminder_registry() -> ReminderRegistry {
    Arc::new(Mutex::new(Vec::new()))
}

/// Register an injected reminder's identity and build the message with it.
fn register_and_build(text: String, registry: &ReminderRegistry) -> AgentMessage {
    let message = UserMessage::new(UserContent::Text(text), now_ms());
    if let Ok(mut registered) = registry.lock() {
        registered.push(message.clone());
    }
    AgentMessage::User(message)
}

/// Whether `message` is a reminder this harness injected this run (never
/// persisted). Anything else — including a user message that merely *looks*
/// like a reminder, or one that merely shares its millisecond — is durable
/// and must survive compaction.
pub fn is_injected_reminder(message: &AgentMessage, registry: &ReminderRegistry) -> bool {
    let Ok(registered) = registry.lock() else {
        // A poisoned registry must not fail closed (dropping real history).
        return false;
    };
    match message {
        AgentMessage::User(user) => registered.contains(user),
        _ => false,
    }
}

/// A reminder the harness wants injected before the next assistant request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reminder {
    /// Periodic working-state refresh.
    StatusCheck { turn: u32, interval: u32 },
    /// The same tool calls (name + arguments) repeated in a row.
    RepeatCall { tools: String, turns: u32 },
}

impl Reminder {
    /// The injected message text. Kept to a few lines: this is a nudge, not a
    /// plan template, and it is re-sent every `interval` turns.
    pub fn message(&self) -> String {
        match self {
            Self::StatusCheck { turn, interval } => format!(
                "[working-state check] Turn {turn} of this run (checks every {interval}). \
                 In ONE short line — not a full plan restatement — state: the goal, the step \
                 you are on now, and the next concrete action. Then continue working."
            ),
            Self::RepeatCall { tools, turns } => format!(
                "[repeat notice] You called {tools} with identical arguments {turns} turns in \
                 a row. If the results are the same, change your approach instead of retrying \
                 the identical call. If the repetition is intentional, continue."
            ),
        }
    }
}

/// Counts tool-bearing turns and identical-call streaks for one run.
///
/// The harness builds one per run and feeds it each tool-bearing assistant
/// turn via the `after_tool_results` wrapper.
#[derive(Debug, Default)]
pub struct WorkingStateReminder {
    /// Status-check interval in tool-bearing turns (`0` = off).
    interval: u32,
    /// Whether the repeat-call notice is enabled.
    repeat_reminder: bool,
    /// Tool-bearing turns observed so far.
    turns: u32,
    /// The previous turn's tool-call fingerprint and how many turns in a row
    /// it has matched. Reset after a notice fires (one notice per streak).
    streak: Option<(String, u32)>,
}

impl WorkingStateReminder {
    /// Interval in tool-bearing turns (`0` = status checks off).
    pub fn new(interval: u32, repeat_reminder: bool) -> Self {
        Self {
            interval,
            repeat_reminder,
            turns: 0,
            streak: None,
        }
    }

    /// Build from the environment (see the module docs for the variables).
    pub fn from_env() -> Self {
        let interval = match std::env::var(WORKING_STATE_INTERVAL_ENV) {
            Ok(raw) => raw.trim().parse::<u32>().unwrap_or(0),
            Err(_) => 0,
        };
        let repeat_reminder = match std::env::var(REPEAT_TOOL_REMINDER_ENV) {
            Ok(raw) => matches!(
                raw.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "on" | "yes"
            ),
            Err(_) => false,
        };
        Self::new(interval, repeat_reminder)
    }

    /// Whether any reminder is active at all.
    pub fn is_enabled(&self) -> bool {
        self.interval > 0 || self.repeat_reminder
    }

    /// Record one tool-bearing assistant turn and report the reminder (if any)
    /// to inject before the next request. Repeat notices take priority over
    /// status checks; only one reminder is injected per turn.
    pub fn observe_turn(&mut self, message: &AssistantMessage) -> Option<Reminder> {
        self.turns = self.turns.saturating_add(1);
        let fingerprint = tool_call_fingerprint(message);

        if let Some(fingerprint) = fingerprint {
            let streak = match &mut self.streak {
                Some((prev, count)) if *prev == fingerprint => {
                    *count = count.saturating_add(1);
                    *count
                }
                _ => {
                    self.streak = Some((fingerprint, 1));
                    1
                }
            };
            if self.repeat_reminder && streak >= REPEAT_TRIGGER_TURNS {
                // Reset so a continued streak notifies again only after another
                // full streak, and one notice maps to one nudge.
                self.streak = None;
                let tools = tool_call_names(message);
                return Some(Reminder::RepeatCall {
                    tools,
                    turns: streak,
                });
            }
        } else {
            self.streak = None;
        }

        if self.interval > 0 && self.turns % self.interval == 0 {
            return Some(Reminder::StatusCheck {
                turn: self.turns,
                interval: self.interval,
            });
        }
        None
    }
}

/// Fingerprint of one turn's tool calls: each `name:arguments` pair joined in
/// order. `None` when the turn made no tool calls (nothing to compare).
fn tool_call_fingerprint(message: &AssistantMessage) -> Option<String> {
    let calls: Vec<String> = message
        .content
        .iter()
        .filter_map(|content| match content {
            Content::ToolCall(call) => {
                Some(format!("{}:{}", call.name, call.arguments))
            }
            _ => None,
        })
        .collect();
    if calls.is_empty() {
        None
    } else {
        Some(calls.join("|"))
    }
}

/// Human-readable tool names for the repeat notice ("bash and read").
fn tool_call_names(message: &AssistantMessage) -> String {
    let names: Vec<&str> = message
        .content
        .iter()
        .filter_map(|content| match content {
            Content::ToolCall(call) => Some(call.name.as_str()),
            _ => None,
        })
        .collect();
    match names.len() {
        0 => "tools".to_string(),
        1 => format!("`{}`", names[0]),
        _ => format!("`{}`", names.join("`, `")),
    }
}

/// Wrap an existing `after_tool_results` chain with working-state reminders.
///
/// The inner hook (compaction, context delivery, …) runs first; its context
/// update — if any — is what the reminder is appended to, so a compaction and
/// a reminder in the same turn compose instead of racing. Model /
/// thinking-level updates from the inner hook are passed through unchanged.
pub fn wrap_after_tool_results(
    inner: Option<AfterToolResults>,
    state: Arc<Mutex<WorkingStateReminder>>,
    registry: ReminderRegistry,
) -> AfterToolResults {
    Arc::new(move |ctx: ShouldStopAfterTurnContext<'_>| {
        // Synchronous pre-check, ZERO clones: the reminder decision only
        // needs the assistant message, and on the (overwhelmingly common)
        // no-reminder turn we hand `ctx` straight to the inner hook — no
        // AgentContext/new_messages snapshot at all. Only a turn that actually
        // fires a reminder pays for the snapshot below.
        let reminder = state
            .lock()
            .ok()
            .and_then(|mut reminder| reminder.observe_turn(ctx.message));
        match reminder {
            None => match inner.as_ref() {
                // Transparent pipe: the inner hook (compaction, boundary
                // delivery) gets the borrowed ctx unchanged.
                Some(hook) => hook(ctx),
                None => Box::pin(async { None }),
            },
            Some(reminder) => {
                // Snapshot every field the inner hook may read BEFORE the
                // async block: `ctx` borrows cannot live inside a `'static`
                // future (same pattern as `context_boundary_hook`).
                let message = ctx.message.clone();
                let tool_results = ctx.tool_results.to_vec();
                let new_messages = ctx.new_messages.to_vec();
                let context_snapshot = ctx.context.clone();
                let inner = inner.clone();
                let registry = Arc::clone(&registry);
                Box::pin(async move {
                    let inner_update = match inner {
                        Some(hook) => {
                            hook(ShouldStopAfterTurnContext {
                                message: &message,
                                tool_results: &tool_results,
                                context: &context_snapshot,
                                new_messages: &new_messages,
                            })
                            .await
                        }
                        None => None,
                    };

                    // Base = the inner hook's replacement context if it made one,
                    // otherwise the loop's current context. The reminder rides on
                    // top so it survives a same-turn compaction.
                    let mut context = inner_update
                        .as_ref()
                        .and_then(|update| update.context.clone())
                        .unwrap_or(context_snapshot);
                    context
                        .messages
                        .push(register_and_build(reminder.message(), &registry));

                    Some(match inner_update {
                        Some(mut update) => {
                            update.context = Some(context);
                            update
                        }
                        None => AgentLoopTurnUpdate {
                            context: Some(context),
                            model: None,
                            thinking_level: None,
                        },
                    })
                })
            }
        }
    })
}

/// Current time in unix milliseconds.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rpi_ai::types::{Content, StopReason, ToolCall, ToolCallType};

    fn assistant_with_calls(calls: &[(&str, &str)]) -> AssistantMessage {
        let content = calls
            .iter()
            .map(|(name, args)| {
                Content::ToolCall(ToolCall {
                    kind: ToolCallType::default(),
                    id: "call_1".into(),
                    name: (*name).to_string(),
                    arguments: serde_json::from_str(args).unwrap(),
                    thought_signature: None,
                    namespace: None,
                })
            })
            .collect();
        let mut message = AssistantMessage::empty(
            rpi_ai::Api::OpenaiCompletions,
            "provider",
            "model",
            0,
        );
        message.content = content;
        message.stop_reason = StopReason::ToolUse;
        message
    }

    fn text_assistant() -> AssistantMessage {
        let mut message = AssistantMessage::empty(
            rpi_ai::Api::OpenaiCompletions,
            "provider",
            "model",
            0,
        );
        message.content = vec![Content::text("done")];
        message.stop_reason = StopReason::Stop;
        message
    }

    #[test]
    fn status_check_fires_every_interval_turns() {
        let mut reminder = WorkingStateReminder::new(3, false);
        for turn in 1..=6 {
            let fired = reminder.observe_turn(&assistant_with_calls(&[(
                "bash",
                r#"{"command":"ls"}"#,
            )]));
            if turn % 3 == 0 {
                assert_eq!(
                    fired,
                    Some(Reminder::StatusCheck { turn, interval: 3 }),
                    "turn {turn}"
                );
            } else {
                assert_eq!(fired, None, "turn {turn}");
            }
        }
    }

    #[test]
    fn repeat_notice_fires_on_the_third_identical_turn_then_resets() {
        let mut reminder = WorkingStateReminder::new(0, true);
        let message = assistant_with_calls(&[("bash", r#"{"command":"ls"}"#)]);
        assert_eq!(reminder.observe_turn(&message), None);
        assert_eq!(reminder.observe_turn(&message), None);
        assert_eq!(
            reminder.observe_turn(&message),
            Some(Reminder::RepeatCall {
                tools: "`bash`".into(),
                turns: 3
            })
        );
        // The streak reset: two more identical turns stay quiet, the third
        // fires again.
        assert_eq!(reminder.observe_turn(&message), None);
        assert_eq!(reminder.observe_turn(&message), None);
        assert!(matches!(
            reminder.observe_turn(&message),
            Some(Reminder::RepeatCall { .. })
        ));
    }

    #[test]
    fn different_arguments_break_the_streak() {
        let mut reminder = WorkingStateReminder::new(0, true);
        assert_eq!(
            reminder.observe_turn(&assistant_with_calls(&[(
                "bash",
                r#"{"command":"ls"}"#
            )])),
            None
        );
        assert_eq!(
            reminder.observe_turn(&assistant_with_calls(&[(
                "bash",
                r#"{"command":"ls -a"}"#
            )])),
            None
        );
        // Different args restarted the streak at 1, so even a third turn of
        // the *second* variant has only run twice.
        assert_eq!(
            reminder.observe_turn(&assistant_with_calls(&[(
                "bash",
                r#"{"command":"ls -a"}"#
            )])),
            None
        );
    }

    #[test]
    fn repeat_takes_priority_over_status_check() {
        let mut reminder = WorkingStateReminder::new(1, true);
        let message = assistant_with_calls(&[("bash", r#"{"command":"ls"}"#)]);
        let _ = reminder.observe_turn(&message);
        let _ = reminder.observe_turn(&message);
        assert!(matches!(
            reminder.observe_turn(&message),
            Some(Reminder::RepeatCall { .. })
        ));
    }

    #[test]
    fn turns_without_tool_calls_reset_the_streak_and_still_count() {
        // Defensive: the real loop only invokes the hook on tool-bearing
        // turns, but the counter must stay sane if that ever changes — text
        // turns count for the interval, break the streak, and never repeat-notify.
        let mut reminder = WorkingStateReminder::new(4, true);
        let call = assistant_with_calls(&[("bash", r#"{"command":"ls"}"#)]);
        let _ = reminder.observe_turn(&call); // turn 1, streak = 1
        assert_eq!(reminder.observe_turn(&text_assistant()), None); // turn 2
        assert_eq!(reminder.observe_turn(&text_assistant()), None); // turn 3
        assert_eq!(
            reminder.observe_turn(&text_assistant()),
            Some(Reminder::StatusCheck { turn: 4, interval: 4 })
        );
        // The streak restarted after the break: two identical turns stay
        // quiet (streak 1 then 2, below the trigger of 3).
        let _ = reminder.observe_turn(&call); // turn 5, streak restarts at 1
        assert_eq!(reminder.observe_turn(&call), None); // turn 6, streak = 2
    }

    #[test]
    fn from_env_defaults_to_disabled() {
        let previous_interval =
            std::env::var(WORKING_STATE_INTERVAL_ENV).ok();
        let previous_repeat = std::env::var(REPEAT_TOOL_REMINDER_ENV).ok();
        let restore = || {
            match previous_interval {
                Some(ref v) => std::env::set_var(WORKING_STATE_INTERVAL_ENV, v),
                None => std::env::remove_var(WORKING_STATE_INTERVAL_ENV),
            }
            match previous_repeat {
                Some(ref v) => std::env::set_var(REPEAT_TOOL_REMINDER_ENV, v),
                None => std::env::remove_var(REPEAT_TOOL_REMINDER_ENV),
            }
        };

        std::env::remove_var(WORKING_STATE_INTERVAL_ENV);
        std::env::remove_var(REPEAT_TOOL_REMINDER_ENV);
        let reminder = WorkingStateReminder::from_env();
        assert!(!reminder.is_enabled());

        std::env::set_var(WORKING_STATE_INTERVAL_ENV, "10");
        std::env::set_var(REPEAT_TOOL_REMINDER_ENV, "1");
        let reminder = WorkingStateReminder::from_env();
        assert!(reminder.is_enabled());

        std::env::set_var(WORKING_STATE_INTERVAL_ENV, "not-a-number");
        std::env::set_var(REPEAT_TOOL_REMINDER_ENV, "0");
        let reminder = WorkingStateReminder::from_env();
        assert!(!reminder.is_enabled());

        restore();
    }

    #[test]
    fn messages_are_short_nudges() {
        let status = Reminder::StatusCheck {
            turn: 10,
            interval: 10,
        }
        .message();
        assert!(status.contains("ONE short line"), "{status}");
        assert!(status.contains("not a full plan"), "{status}");

        let repeat = Reminder::RepeatCall {
            tools: "`bash`".into(),
            turns: 3,
        }
        .message();
        assert!(repeat.contains("identical arguments"), "{repeat}");
        assert!(repeat.contains("change your approach"), "{repeat}");
    }

    #[test]
    fn identity_is_the_full_injected_message() {
        use rpi_agent::message::AgentMessage;

        // A user message that merely LOOKS like a reminder (asking what the
        // notice means) must never be treated as transient — this was the
        // prefix-filter bug the first review found.
        let lookalike = AgentMessage::User(UserMessage::new(
            UserContent::Text(
                "[repeat notice] 这条提示是什么意思?请帮我关闭重复提醒。".into(),
            ),
            123_456,
        ));
        let registry = reminder_registry();
        assert!(
            !is_injected_reminder(&lookalike, &registry),
            "a lookalike user message must be durable"
        );

        // Register a full message; only THAT exact message is transient.
        let injected_message = UserMessage::new(
            UserContent::Text("[repeat notice] You called bash...".into()),
            999,
        );
        registry.lock().unwrap().push(injected_message.clone());
        let injected = AgentMessage::User(injected_message.clone());
        assert!(is_injected_reminder(&injected, &registry));

        // The second review's exact case: a real user message sharing the
        // SAME MILLISECOND (timestamp-only matching would drop it), with
        // different content — must stay durable.
        let same_ms_other_text = AgentMessage::User(UserMessage::new(
            UserContent::Text("完全不同的真实用户输入".into()),
            999,
        ));
        assert!(
            !is_injected_reminder(&same_ms_other_text, &registry),
            "same-millisecond real input must stay durable"
        );

        // Same text, different timestamp: not the injected message either.
        let same_text_other_time = AgentMessage::User(UserMessage::new(
            UserContent::Text("[repeat notice] You called bash...".into()),
            1000,
        ));
        assert!(
            !is_injected_reminder(&same_text_other_time, &registry),
            "same text at a different time stays durable"
        );

        // An empty registry (reminders disabled) matches nothing at all.
        let empty = reminder_registry();
        assert!(!is_injected_reminder(&injected, &empty));
    }

    use rpi_agent::types::AgentContext;

    async fn call_wrapped(
        wrapped: &AfterToolResults,
        message: &AssistantMessage,
    ) -> Option<AgentLoopTurnUpdate> {
        let context = AgentContext {
            system_prompt: "sys".into(),
            messages: vec![AgentMessage::User(rpi_ai::types::UserMessage::new(
                UserContent::Text("base".into()),
                0,
            ))],
            tools: Vec::new(),
        };
        let new_messages: Vec<AgentMessage> = Vec::new();
        wrapped(ShouldStopAfterTurnContext {
            message,
            tool_results: &[],
            context: &context,
            new_messages: &new_messages,
        })
        .await
    }

    fn injected_text(update: &AgentLoopTurnUpdate) -> String {
        let context = update.context.as_ref().expect("context update");
        match context.messages.last() {
            Some(AgentMessage::User(user)) => match &user.content {
                UserContent::Text(text) => text.clone(),
                other => panic!("expected text content, got {other:?}"),
            },
            other => panic!("expected an injected user message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn wrap_appends_the_reminder_after_the_inner_context() {
        let state = Arc::new(Mutex::new(WorkingStateReminder::new(1, false)));
        let wrapped = wrap_after_tool_results(None, state, reminder_registry());
        let message = assistant_with_calls(&[("bash", r#"{"command":"ls"}"#)]);

        let update = call_wrapped(&wrapped, &message)
            .await
            .expect("a status check must produce an update");
        let text = injected_text(&update);
        assert!(text.contains("[working-state check]"), "{text}");
        // The base context is preserved: the reminder rides ON TOP, it does
        // not replace history.
        let context = update.context.as_ref().unwrap();
        assert!(context.messages.len() >= 2);
    }

    #[tokio::test]
    async fn wrap_passes_through_when_no_reminder_fires() {
        // Disabled reminder: the wrapper must be a transparent pipe.
        let state = Arc::new(Mutex::new(WorkingStateReminder::new(0, false)));
        let inner: AfterToolResults = Arc::new(|_ctx: ShouldStopAfterTurnContext<'_>| {
            Box::pin(async { None })
        });
        let wrapped = wrap_after_tool_results(Some(inner), state, reminder_registry());
        let message = assistant_with_calls(&[("bash", r#"{"command":"ls"}"#)]);
        assert!(call_wrapped(&wrapped, &message).await.is_none());
    }

    #[tokio::test]
    async fn wrap_composes_with_an_inner_context_update() {
        // An inner hook replaced the context (e.g. compaction): the reminder
        // must land on the REPLACEMENT, not on the snapshot it superseded.
        let inner: AfterToolResults = Arc::new(|_ctx: ShouldStopAfterTurnContext<'_>| {
            Box::pin(async {
                Some(AgentLoopTurnUpdate {
                    context: Some(AgentContext {
                        system_prompt: "sys".into(),
                        messages: vec![AgentMessage::User(rpi_ai::types::UserMessage::new(
                            UserContent::Text("compacted".into()),
                            0,
                        ))],
                        tools: Vec::new(),
                    }),
                    model: None,
                    thinking_level: None,
                })
            })
        });
        let state = Arc::new(Mutex::new(WorkingStateReminder::new(1, false)));
        let wrapped = wrap_after_tool_results(Some(inner), state, reminder_registry());
        let message = assistant_with_calls(&[("bash", r#"{"command":"ls"}"#)]);

        let update = call_wrapped(&wrapped, &message)
            .await
            .expect("status check fires");
        let context = update.context.as_ref().unwrap();
        assert_eq!(context.messages.len(), 2);
        let text = injected_text(&update);
        assert!(text.contains("[working-state check]"), "{text}");
    }
}
