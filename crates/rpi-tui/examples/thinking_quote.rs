//! 临时预览:Everforest thinking 竖线包裹效果(真实渲染路径,非模拟)。
//! 走 assistant_message.rs → markdown.rs 的 blockquote 路径,所见即 TUI 所得。
//!
//! Run: cargo run -p rpi-tui --example thinking_quote

use rpi_tui::component::Component;
use rpi_tui::{apply_theme_preset, AssistantBlock, AssistantMessageComponent, ThemePreset};

fn main() {
    apply_theme_preset(ThemePreset::Everforest);

    // 版本横幅:区分终端回滚缓冲里可能的上一版输出(竖线+斜体)。
    // v2 = 竖线 + 无斜体(去字形斑驳)。重跑时先确认看到这一行。
    println!("\x1b[38;2;157;169;160mv2 no-italic build(竖线+无斜体)—以下为当前渲染—\x1b[0m");

    let thinking = "\
先核对配置读取与保存路径:改动只在渲染层给 thinking 套引用块路径,\
markdown 渲染器现成的 push_wrapped_with_prefix 保证折行后竖线连续,零新框架。

左侧竖线是结构性标记,与正文的区分不依赖色觉;内容保持 #97a49b,\
连续阅读舒适度不变。这一段较长,专门验证折行时竖线是否每行都在——\
如果每行开头都有竖线,说明 wrap 前缀机制可靠。也包含一个 \
`inline code` 验证内联代码之后斜体与颜色恢复。";

    let answer = "\
已完成——thinking 竖线包裹:

- 正文 #ccd4c8 不变,竖线色 grey0 #7a8478
- thinking 内容保持 #97a49b(引用路径承载)
- 结构 + 亮度双重区分(无斜体)

```rust
let ok = true;
```";

    let msg = AssistantMessageComponent::default();
    msg.update_blocks(&[
        AssistantBlock::Thinking(thinking.to_string()),
        AssistantBlock::Text(answer.to_string()),
    ]);
    for line in msg.render(88) {
        println!("{line}");
    }
}
