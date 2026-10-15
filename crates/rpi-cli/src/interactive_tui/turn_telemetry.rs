//! Turn telemetry — pi-open-tui `telemetry.ts` 的移植。
//!
//! 每个 turn（一次 assistant 响应 + 其工具调用/结果）结束时输出一行
//! transient 遥测：`TPS 42.5 tok/s | ~ TTFT 1.2s | + 29.7s | ↑ 567 | ↓ 1.2k |
//! ! stall 1x/4.3s | $ 3.60/M`。
//!
//! 口径与 telemetry.ts 一致：
//! - TPS = provider 上报的 assistant 输出 token / 生成时间（turn_start →
//!   最后一个 assistant message_end，含 TTFT、隐藏推理与 stall；turn 间的
//!   工具执行不计入）
//! - TTFT = turn 内首个流式 delta（text/thinking/toolcall）到达的耗时
//! - stall：流式 delta 间隔 ≥ 1s 计一次（同一间隔只计一次）
//! - input 口径与 /session 的 "uncached" 一致：input + cacheWrite（全新
//!   近全价内容），cacheRead 为折价的重复内容，单列
//! - $ / M = 该 turn 的 list-price 费率（usage.cost.total / totalTokens）

use std::sync::Mutex;
use std::time::Instant;

use rpi_agent::{AgentEvent, AgentMessage};
use rpi_ai::types::{AssistantMessage, AssistantMessageEvent};

/// 流式 delta 间隔达到该值计一次 stall（telemetry.ts 的 1000ms）。
const STALL_THRESHOLD_MS: u64 = 1000;

type Clock = Box<dyn Fn() -> Instant + Send + Sync>;

/// 单个 turn 的遥测结果。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TurnTelemetry {
    /// 输出 token / 生成时间（tok/s）。生成时间或输出 token 缺失时为 None。
    pub tps: Option<f64>,
    /// turn_start → 首 delta 的毫秒数。
    pub ttft_ms: u128,
    /// turn_start → turn_end 的总毫秒数。
    pub total_ms: u128,
    /// uncached 输入 token（input + cacheWrite）。
    pub input_tokens: i64,
    /// 输出 token。
    pub output_tokens: i64,
    /// 折价读取的重复内容 token（单列，不计入 input）。
    pub cache_read_tokens: i64,
    /// stall 次数。
    pub stall_count: u32,
    /// stall 累计毫秒数。
    pub stall_ms: u64,
    /// list-price 费率（$/M tokens）。token 或费用缺失时为 None。
    pub rate_usd_per_m: Option<f64>,
}

/// 当前 turn 的计时与用量累积。
#[derive(Debug)]
struct TurnState {
    start: Instant,
    first_token: Option<Instant>,
    /// turn_start → 最后一个 assistant message_end（生成时间）。
    generation: Option<Instant>,
    stall_ms: u64,
    stall_count: u32,
    /// 当前流式消息的上一个 delta 时刻（message 边界重置）。
    last_delta: Option<Instant>,
    in_stall: bool,
    /// 本 turn 所有 assistant 消息的 usage 累积。
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    total_tokens: i64,
    cost: f64,
    messages: usize,
}

/// 单会话 turn 遥测追踪器。事件 drain 与渲染 tick 可能并发访问，内部用
/// [`Mutex`] 串行化。
pub(super) struct TurnTelemetryTracker {
    turn: Mutex<Option<TurnState>>,
    clock: Clock,
}

impl TurnTelemetryTracker {
    /// 用真实时钟构造。
    pub(super) fn new() -> Self {
        Self::with_clock(Box::new(Instant::now))
    }

    /// 测试用：注入受控时钟。
    pub(super) fn with_clock(clock: Clock) -> Self {
        Self {
            turn: Mutex::new(None),
            clock,
        }
    }

    /// 喂入一个事件。`TurnEnd` 且该 turn 产生了可测的遥测（有首 delta、
    /// 至少一条 assistant 消息）时返回 [`TurnTelemetry`]。
    pub(super) fn record(&self, event: &AgentEvent) -> Option<TurnTelemetry> {
        let now = (self.clock)();
        let mut turn = self.turn.lock().ok()?;
        match event {
            AgentEvent::TurnStart => {
                *turn = Some(TurnState {
                    start: now,
                    first_token: None,
                    generation: None,
                    stall_ms: 0,
                    stall_count: 0,
                    last_delta: None,
                    in_stall: false,
                    input: 0,
                    output: 0,
                    cache_read: 0,
                    cache_write: 0,
                    total_tokens: 0,
                    cost: 0.0,
                    messages: 0,
                });
                None
            }
            AgentEvent::MessageStart { message } => {
                if let AgentMessage::Assistant(_) = message {
                    if let Some(state) = turn.as_mut() {
                        // message 边界重置流式状态（工具执行期不计 stall）。
                        state.last_delta = None;
                        state.in_stall = false;
                    }
                }
                None
            }
            AgentEvent::MessageUpdate {
                assistant_message_event,
                ..
            } => {
                let state = turn.as_mut()?;
                if !is_stream_delta(assistant_message_event) {
                    return None;
                }
                if state.first_token.is_none() {
                    state.first_token = Some(now);
                    state.last_delta = Some(now);
                    return None;
                }
                let last = state.last_delta.unwrap_or(now);
                let gap_ms = now.duration_since(last).as_millis() as u64;
                if gap_ms >= STALL_THRESHOLD_MS {
                    if !state.in_stall {
                        state.stall_count += 1;
                    }
                    state.in_stall = true;
                    state.stall_ms += gap_ms;
                } else {
                    state.in_stall = false;
                }
                state.last_delta = Some(now);
                None
            }
            AgentEvent::MessageEnd { message } => {
                if let AgentMessage::Assistant(a) = message {
                    let state = turn.as_mut()?;
                    state.generation = Some(now);
                    state.messages += 1;
                    accumulate_usage(state, a);
                }
                None
            }
            AgentEvent::TurnEnd { .. } => {
                let state = turn.take()?;
                if state.first_token.is_none() || state.messages == 0 {
                    return None;
                }
                Some(TurnTelemetry {
                    tps: state
                        .generation
                        .map(|g| {
                            let ms = g.duration_since(state.start).as_millis() as f64;
                            if ms > 0.0 && state.output > 0 {
                                Some((state.output as f64 / (ms / 1000.0) * 10.0).round() / 10.0)
                            } else {
                                None
                            }
                        })
                        .flatten(),
                    ttft_ms: state
                        .first_token
                        .unwrap_or(state.start)
                        .duration_since(state.start)
                        .as_millis(),
                    total_ms: now.duration_since(state.start).as_millis(),
                    input_tokens: state.input + state.cache_write,
                    output_tokens: state.output,
                    cache_read_tokens: state.cache_read,
                    stall_count: state.stall_count,
                    stall_ms: state.stall_ms,
                    rate_usd_per_m: if state.cost > 0.0 && state.total_tokens > 0 {
                        Some(
                            (state.cost / (state.total_tokens as f64 / 1_000_000.0) * 100.0)
                                .round()
                                / 100.0,
                        )
                    } else {
                        None
                    },
                })
            }
            _ => None,
        }
    }
}

/// 只有携带非空 delta 的 text/thinking/toolcall 事件计入首 token 与 stall
/// 计时（telemetry.ts 的 `delta.length === 0` 过滤）。
fn is_stream_delta(event: &AssistantMessageEvent) -> bool {
    match event {
        AssistantMessageEvent::TextDelta { delta, .. }
        | AssistantMessageEvent::ThinkingDelta { delta, .. }
        | AssistantMessageEvent::ToolCallDelta { delta, .. } => !delta.is_empty(),
        _ => false,
    }
}

fn accumulate_usage(state: &mut TurnState, a: &AssistantMessage) {
    let usage = &a.usage;
    state.input += usage.input;
    state.output += usage.output;
    state.cache_read += usage.cache_read;
    state.cache_write += usage.cache_write;
    state.total_tokens += usage.total_tokens;
    state.cost += usage.cost.total;
}

/// 人类可读的时长（telemetry.ts 的 formatTurnDuration）：< 60s → `1.2s`，
/// 否则 → `1m02s`。
fn format_duration(ms: u64) -> String {
    if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{}m{:02}s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

/// token 缩写（footer 的 k/M 口径）。
fn format_tokens(count: i64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}k", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

/// 组装 turn 遥测行（纯文本；着色由调用方决定）。
pub(super) fn format_turn_telemetry(t: &TurnTelemetry) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(match t.tps {
        Some(tps) => format!("TPS {tps:.1} tok/s"),
        None => "TPS —".to_string(),
    });
    parts.push(format!("~ TTFT {}", format_duration(t.ttft_ms as u64)));
    parts.push(format!("+ {}", format_duration(t.total_ms as u64)));
    if t.input_tokens > 0 {
        parts.push(format!("↑ {}", format_tokens(t.input_tokens)));
    }
    if t.output_tokens > 0 {
        parts.push(format!("↓ {}", format_tokens(t.output_tokens)));
    }
    if t.stall_ms > 0 {
        parts.push(format!(
            "! stall {}x/{}",
            t.stall_count,
            format_duration(t.stall_ms)
        ));
    }
    if let Some(rate) = t.rate_usd_per_m {
        parts.push(format!("$ ${rate:.2}/M"));
    }
    parts.join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rpi_ai::types::{
        Api, Content, TextContent, TextContentType, Usage, UsageCost, UserMessage,
    };
    use std::sync::Arc;
    use std::time::Duration;

    /// 可手动推进的假时钟。
    struct FakeClock {
        now: Mutex<Instant>,
    }

    impl FakeClock {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                now: Mutex::new(Instant::now()),
            })
        }

        fn advance(&self, ms: u64) {
            let mut now = self.now.lock().unwrap();
            *now += Duration::from_millis(ms);
        }

        fn tracker(self: &Arc<Self>) -> TurnTelemetryTracker {
            let clock = self.clone();
            TurnTelemetryTracker::with_clock(Box::new(move || *clock.now.lock().unwrap()))
        }
    }

    /// 构造一个 assistant message_end 事件（带 usage）。
    fn assistant_done_event(usage: Usage) -> AgentEvent {
        let mut message = AssistantMessage::empty(Api::Faux, "faux", "faux", 0);
        message.content = vec![Content::Text(TextContent {
            kind: TextContentType,
            text: "done".to_string(),
            text_signature: None,
        })];
        message.usage = usage;
        AgentEvent::MessageEnd {
            message: AgentMessage::Assistant(Box::new(message)),
        }
    }

    /// 构造一个非空 text delta 事件。
    fn text_delta() -> AgentEvent {
        let partial = Arc::new(AssistantMessage::empty(Api::Faux, "faux", "faux", 0));
        AgentEvent::MessageUpdate {
            message: partial.clone(),
            assistant_message_event: AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "tok".to_string(),
                partial,
            },
        }
    }

    /// 构造一个 turn_end 事件（载荷与遥测无关）。
    fn turn_end() -> AgentEvent {
        AgentEvent::TurnEnd {
            message: AgentMessage::User(UserMessage::new("", 0)),
            tool_results: vec![],
        }
    }

    #[test]
    fn turn_end_collects_tps_ttft_tokens_and_cost() {
        let clock = FakeClock::new();
        let tracker = clock.tracker();

        assert!(tracker.record(&AgentEvent::TurnStart).is_none());

        clock.advance(1200); // TTFT 1.2s
        assert!(tracker.record(&text_delta()).is_none());

        clock.advance(28_500); // 生成结束于 +29.7s
        assert!(tracker
            .record(&assistant_done_event(Usage {
                input: 500,
                output: 200,
                cache_read: 100,
                cache_write: 67,
                total_tokens: 867,
                cost: UsageCost {
                    total: 0.03,
                    ..Default::default()
                },
                ..Usage::zero()
            }))
            .is_none());

        clock.advance(200); // turn 结束于 +29.9s
        let telemetry = tracker
            .record(&turn_end())
            .expect("turn end must collect telemetry");

        assert_eq!(telemetry.ttft_ms, 1200);
        assert_eq!(telemetry.total_ms, 29_900);
        assert_eq!(telemetry.input_tokens, 500 + 67);
        assert_eq!(telemetry.output_tokens, 200);
        assert_eq!(telemetry.cache_read_tokens, 100);
        // TPS = 200 / 29.7s ≈ 6.7
        assert_eq!(telemetry.tps, Some(6.7));
        // $ / M = 0.03 / (867 / 1M) ≈ 34.60
        assert_eq!(telemetry.rate_usd_per_m, Some(34.6));
        assert_eq!(telemetry.stall_count, 0);

        let line = format_turn_telemetry(&telemetry);
        assert!(line.contains("TPS 6.7 tok/s"), "{line}");
        assert!(line.contains("TTFT 1.2s"), "{line}");
        assert!(line.contains("+ 29.9s"), "{line}");
        assert!(line.contains("↑ 567"), "{line}");
        assert!(line.contains("↓ 200"), "{line}");
        assert!(!line.contains("stall"), "{line}");
        assert!(line.contains("$ $34.60/M"), "{line}");
    }

    #[test]
    fn long_gap_counts_as_stall_once() {
        let clock = FakeClock::new();
        let tracker = clock.tracker();

        tracker.record(&AgentEvent::TurnStart);
        tracker.record(&text_delta()); // 首 token，t=0

        clock.advance(4_300); // 4.3s 无输出 → stall 1x / 4.3s
        tracker.record(&text_delta());
        // 后续密集 delta 不再计 stall
        clock.advance(100);
        tracker.record(&text_delta());

        tracker.record(&assistant_done_event(Usage {
            output: 10,
            ..Usage::zero()
        }));

        let telemetry = tracker.record(&turn_end()).expect("telemetry");

        assert_eq!(telemetry.stall_count, 1);
        assert_eq!(telemetry.stall_ms, 4_300);
        let line = format_turn_telemetry(&telemetry);
        assert!(line.contains("! stall 1x/4.3s"), "{line}");
    }

    #[test]
    fn turn_without_streaming_yields_no_telemetry() {
        let clock = FakeClock::new();
        let tracker = clock.tracker();

        tracker.record(&AgentEvent::TurnStart);
        // 无任何 delta 直接结束（例如 provider 错误）。
        assert!(tracker.record(&turn_end()).is_none());
    }

    #[test]
    fn multi_message_turn_accumulates_usage() {
        let clock = FakeClock::new();
        let tracker = clock.tracker();

        tracker.record(&AgentEvent::TurnStart);
        tracker.record(&text_delta());
        tracker.record(&assistant_done_event(Usage {
            output: 100,
            total_tokens: 100,
            ..Usage::zero()
        }));
        // 工具循环后的第二段回复。
        tracker.record(&text_delta());
        clock.advance(500);
        tracker.record(&assistant_done_event(Usage {
            output: 50,
            total_tokens: 50,
            input: 10,
            ..Usage::zero()
        }));

        let telemetry = tracker.record(&turn_end()).expect("telemetry");

        assert_eq!(telemetry.output_tokens, 150);
        assert_eq!(telemetry.input_tokens, 10);
    }

    #[test]
    fn durations_and_tokens_format_compactly() {
        assert_eq!(format_duration(1_234), "1.2s");
        assert_eq!(format_duration(62_000), "1m02s");
        assert_eq!(format_tokens(567), "567");
        assert_eq!(format_tokens(1_200), "1.2k");
        assert_eq!(format_tokens(2_300_000), "2.3M");
    }
}
