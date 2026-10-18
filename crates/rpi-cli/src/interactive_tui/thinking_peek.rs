//! Thinking peek — pi-open-tui `peek.ts` 的移植（单行模式）。
//!
//! 当 hide-thinking 开启时，assistant 消息的 thinking 块只渲染一个静态的
//! `Thinking...` 标签。本模块用推理流的尾部一行替换该标签：模型思考时显示
//! `~ think ⠋ <tail>`（spinner 随流式 delta 推进），正文开始后落定为
//! `~ think ✓`，任务收尾后由调用方恢复原生标签。
//!
//! 状态机语义与 peek.ts 的 `reducePeek` 一致：
//! - idle + thinking        → thinking（spinner 启动）
//! - idle + thinking + text → done（已有正文，不旋转）
//! - thinking + text        → done（正文开始，停转）
//! - done + thinking        → done（不重启 spinner）

use std::sync::Mutex;

use rpi_tui::ansi::strip_ansi;
use rpi_tui::utils::truncate_to_width;

/// Braille spinner 帧（与 `Loader` / editor 工作指示共享观感）。
const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// peek 标签前缀（pi-open-tui 的 `~ think` 标记）。
const PREFIX: &str = "~ think";

/// 原生标签文本（任务收尾后恢复用）。
pub(super) const NATIVE_LABEL: &str = "Thinking...";

/// peek 状态机阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Phase {
    /// 尚无推理流（保持原生标签）。
    #[default]
    Idle,
    /// 模型正在输出推理文本（spinner 旋转）。
    Thinking,
    /// 正文已开始（落定 ✓，不再旋转）。
    Done,
}

#[derive(Debug, Default)]
struct PeekState {
    phase: Phase,
    frame: usize,
    tail: String,
    last_label: String,
}

/// 单会话 thinking-peek 状态机。事件 drain 与渲染 tick 都可能并发访问，
/// 内部用 [`Mutex`] 串行化。
pub(super) struct ThinkingPeek {
    state: Mutex<PeekState>,
}

impl ThinkingPeek {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(PeekState::default()),
        }
    }

    /// 消费一次流式更新。`thinking` 是累积的推理文本，`has_text` 表示正文
    /// 是否已开始。返回应写入 hidden-thinking 标签的新文本；`None` 表示
    /// 标签无需变化（idle 或 label 与上次相同）。
    pub(super) fn update(&self, thinking: &str, has_text: bool, width: usize) -> Option<String> {
        let mut state = self.state.lock().ok()?;
        if !thinking.is_empty() {
            state.tail = sanitize_tail(&peek_tail(thinking));
        }
        if !thinking.is_empty() && state.phase == Phase::Idle {
            state.phase = Phase::Thinking;
        }
        if has_text && state.phase == Phase::Thinking {
            state.phase = Phase::Done;
        }
        state.frame = state.frame.wrapping_add(1);
        emit_label(&mut state, width)
    }

    /// 消息收尾（`message_end`）：把仍处于 thinking 的阶段落定为 done。
    pub(super) fn settle(&self, width: usize) -> Option<String> {
        let mut state = self.state.lock().ok()?;
        if state.phase == Phase::Thinking {
            state.phase = Phase::Done;
        }
        emit_label(&mut state, width)
    }

    /// 任务收尾（`agent_end`）：回到 idle。调用方应把标签恢复为
    /// [`NATIVE_LABEL`]。
    pub(super) fn reset(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.phase = Phase::Idle;
            state.tail.clear();
            state.last_label.clear();
        }
    }

    /// 当前阶段（测试与诊断用）。
    pub(super) fn phase(&self) -> Phase {
        self.state.lock().map(|s| s.phase).unwrap_or(Phase::Idle)
    }
}

/// 计算 label 并与上次比较（只在变化时返回 `Some`）。
fn emit_label(state: &mut PeekState, width: usize) -> Option<String> {
    let label = match state.phase {
        Phase::Thinking => {
            let spin = SPINNER_FRAMES[state.frame % SPINNER_FRAMES.len()];
            let plain = if state.tail.is_empty() {
                format!("{PREFIX} {spin}")
            } else {
                format!("{PREFIX} {spin} {}", state.tail)
            };
            truncate_to_width(&plain, width.max(1), "…")
        }
        Phase::Done => truncate_to_width(&format!("{PREFIX} ✓"), width.max(1), "…"),
        Phase::Idle => return None,
    };
    if label == state.last_label {
        return None;
    }
    state.last_label = label.clone();
    Some(label)
}

/// 提取累积推理文本的最后一个非空逻辑行（空白归一化）。只扫描尾部窗口
/// （~2 KiB），长推理也不会随 token 数线性放大。
fn peek_tail(full: &str) -> String {
    if full.is_empty() {
        return String::new();
    }
    let mut start = full.len().saturating_sub(2048);
    while start > 0 && !full.is_char_boundary(start) {
        start -= 1;
    }
    full[start..]
        .lines()
        .rev()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

/// 模型输出不可信：剥离 ANSI 转义、把控制字符压成空格、修剪首尾空白。
fn sanitize_tail(text: &str) -> String {
    let stripped = strip_ansi(text);
    stripped
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_machine_matches_pi_open_tui() {
        let peek = ThinkingPeek::new();
        assert_eq!(peek.phase(), Phase::Idle);

        // idle + thinking → thinking
        peek.update("step one\nstep two", false, 80);
        assert_eq!(peek.phase(), Phase::Thinking);

        // thinking + text → done
        peek.update("step one\nstep two", true, 80);
        assert_eq!(peek.phase(), Phase::Done);

        // done + more thinking stays done（不重启 spinner）
        peek.update("step one\nstep two\nstep three", false, 80);
        assert_eq!(peek.phase(), Phase::Done);
    }

    #[test]
    fn label_shows_tail_then_checkmark() {
        let peek = ThinkingPeek::new();

        let first = peek
            .update("analyzing\nthe module graph", false, 80)
            .unwrap();
        assert!(first.contains("~ think"), "{first:?}");
        assert!(first.contains("the module graph"), "{first:?}");

        // 正文开始：落定 ✓
        let done = peek
            .update("analyzing\nthe module graph", true, 80)
            .unwrap();
        assert!(done.contains("✓"), "{done:?}");

        // 相同 label 不重复返回（避免每 delta 重建消息组件）
        assert!(peek
            .update("analyzing\nthe module graph", true, 80)
            .is_none());
    }

    #[test]
    fn spinner_frame_advances_per_delta() {
        let peek = ThinkingPeek::new();
        let a = peek.update("thinking", false, 80).unwrap();
        let b = peek.update("thinking", false, 80).unwrap();
        assert_ne!(a, b, "spinner must advance between deltas");
    }

    #[test]
    fn settle_marks_done_and_reset_returns_to_idle() {
        let peek = ThinkingPeek::new();
        peek.update("some reasoning", false, 80);
        assert_eq!(peek.phase(), Phase::Thinking);

        let settled = peek.settle(80).unwrap();
        assert!(settled.contains("✓"));
        assert_eq!(peek.phase(), Phase::Done);

        peek.reset();
        assert_eq!(peek.phase(), Phase::Idle);
        // idle 阶段不再产生 label
        assert!(peek.settle(80).is_none());
    }

    #[test]
    fn idle_update_without_thinking_yields_no_label() {
        let peek = ThinkingPeek::new();
        assert!(peek.update("", false, 80).is_none());
        assert!(peek.update("", true, 80).is_none());
    }

    #[test]
    fn label_is_width_clipped() {
        let peek = ThinkingPeek::new();
        let long = "x".repeat(200);
        let label = peek.update(&long, false, 30).unwrap();
        // 前缀 + spinner + 省略号必须都在宽度内
        assert!(rpi_tui::ansi::visible_width(&label) <= 30, "{label:?}");
        assert!(label.contains("~ think"));
    }

    #[test]
    fn peek_tail_takes_last_nonempty_line() {
        assert_eq!(peek_tail("a\nb\n  \nc"), "c");
        assert_eq!(peek_tail("  \n \n"), "");
        assert_eq!(peek_tail(""), "");
        // 空白归一化
        assert_eq!(peek_tail("one\ntwo   three"), "two three");
    }

    #[test]
    fn sanitize_strips_ansi_and_controls() {
        assert_eq!(sanitize_tail("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(sanitize_tail("a\x08b"), "a b");
    }
}
