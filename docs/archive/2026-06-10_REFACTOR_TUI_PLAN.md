# TUI 彻底重构计划（方案 B）

## 当前状态（已完成）

- ✅ `render()` 签名改为 `&AppState`（纯只读）
- ✅ `frame_count += 1` 和 `rebuild_cache` 移出渲染循环
- ✅ 鼠标 hover 不再依赖渲染时写入的 `main_area_y/height`
- ✅ `render.rs` 拆分为 `render.rs` + `markdown.rs` + `widgets/` 目录
- ✅ Dropdown 实现 `Widget` trait，零 `" ".repeat()`

## 目标架构

```

UI = f(State)
│
├─ render_conversation(area, &AppState)
│ ├─ 根据 scroll 找到可见 CachedBlocks
│ ├─ Layout::vertical 分配每个 block 的 sub-area
│ └─ 对每个 block 调用独立 Widget 渲染
│
└─ CachedConversation
└─ Vec<CachedBlock> （不再是 Vec<Line>）
├─ kind: BlockKind
├─ wrapped_height: usize
└─ subagent_id: Option<String>

```

每个 `CachedBlock` 独立渲染在自己的 `Rect` 内，背景色通过 `buf.set_style(area, bg)` 设置，彻底消灭手动空格填充。

---

## 第一阶段：数据层（state.rs）

### 1.1 替换 CachedConversation

```rust
pub struct CachedBlock {
    pub kind: BlockKind,
    pub wrapped_height: usize,
    pub subagent_id: Option<String>,
}

#[derive(Clone)]
pub enum BlockKind {
    Spacing,
    User(String),
    Thought(String),
    Response(String),
    Tool { name: String, args: String, result: Option<ToolResult> },
    Subagent(SubagentState),
    Notice(String),
    Error(String),
    System(String),
    Working,
}

pub struct CachedConversation {
    pub entry_blocks: Vec<CachedBlock>,
    pub rendered_entry_count: usize,
    pub streaming_blocks: Vec<CachedBlock>,
    pub blocks: Vec<CachedBlock>,
    pub version: u64,
    pub width: u16,
    pub wrapped_height: usize,
    pub last_rebuild: Option<Instant>,
}
```

**删除字段：** `entry_lines`, `streaming_lines`, `lines`, `row_offsets`, `subagent_line_ranges`, `entry_subagent_ranges`

---

## 第二阶段：Widget 层（新建 widgets/blocks.rs）

### 2.1 每个 Block 提供 `to_lines()` 用于高度计算

在 `rebuild_cache` 中，我们需要先得到 `Vec<Line>` 才能用 `estimate_wrapped_rows` 计算 `wrapped_height`。所以每个 block 先提供返回 `Vec<Line>` 的纯函数：

```rust
pub fn user_lines(text: &str, width: usize) -> Vec<Line<'static>>;
pub fn thought_lines(text: &str, width: usize) -> Vec<Line<'static>>;
pub fn response_lines(text: &str, width: usize) -> Vec<Line<'static>>; // 调用 markdown::markdown_to_lines
pub fn tool_lines(name: &str, args: &str, result: &Option<ToolResult>, width: usize) -> Vec<Line<'static>>;
pub fn subagent_lines(sa: &SubagentState, width: usize) -> Vec<Line<'static>>;
pub fn notice_lines(msg: &str, width: usize) -> Vec<Line<'static>>;
pub fn error_lines(e: &str, width: usize) -> Vec<Line<'static>>;
pub fn system_lines(text: &str, width: usize) -> Vec<Line<'static>>;
pub fn working_lines(frame_count: u64) -> Vec<Line<'static>>;
```

### 2.2 每个 Block 实现 `Widget` trait

渲染时不再使用上面的 `to_lines()`（避免重复解析 markdown），而是直接渲染：

```rust
pub struct UserBlock<'a> { text: &'a str, skip: usize }
impl<'a> Widget for UserBlock<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // 1. 整个 area 设置背景色
        buf.set_style(area, Style::default().bg(USER_BG).fg(Color::White));
        // 2. 内部用 Paragraph 渲染文本，支持 scroll 跳过
        let lines: Vec<Line> = self.text.lines().map(Line::raw).collect();
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((self.skip as u16, 0))
            .render(area, buf);
    }
}

// ToolBlock 更复杂：需要把 area 垂直切成 title/content
pub struct ToolBlock<'a> { ... }
impl<'a> Widget for ToolBlock<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let title_h = 3u16;
        let title_area = Rect::new(area.x, area.y, area.width, title_h);
        let content_area = Rect::new(area.x, area.y + title_h, area.width, area.height - title_h);

        buf.set_style(title_area, Style::default().bg(self.title_bg).fg(self.title_fg));
        buf.set_style(content_area, Style::default().bg(CODE_BG));

        // 渲染 title 和 content...
    }
}
```

**关键原则：** 背景色由 `buf.set_style(area, style)` 统一设置，不再在 `Line` 中塞空格。

### 2.3 动画处理

- `WorkingBlock` 接收 `frame_count`，在 `render` 中动态选择 spinner 帧
- `ToolBlock` 接收 `frame_count`，如果 `result == None`，动态替换 `⚙` 为 `◐◓◑◒`

---

## 第三阶段：重写 rebuild_cache（render.rs）

### 3.1 新逻辑

```rust
pub fn rebuild_cache(state: &mut AppState, width: u16) {
    let w = width as usize;
    let entry_count = state.entries.len();

    // ── Entry blocks ──
    if state.cache.rendered_entry_count != entry_count || state.cache.width != width {
        let mut entry_blocks = Vec::new();
        let mut first = true;
        for entry in &state.entries {
            if !first { entry_blocks.push(CachedBlock::spacing()); }
            first = false;
            entry_to_blocks(entry, w, &mut entry_blocks);
        }
        state.cache.entry_blocks = entry_blocks;
        state.cache.rendered_entry_count = entry_count;
    }

    // ── Streaming blocks ──
    let mut streaming_blocks = Vec::new();
    if let Some(ref streaming) = state.streaming {
        for (i, block) in streaming.blocks.iter().enumerate() {
            if i > 0 { streaming_blocks.push(CachedBlock::spacing()); }
            turn_block_to_blocks(block, w, 0, &mut streaming_blocks);
        }
    }
    state.cache.streaming_blocks = streaming_blocks;

    // ── Combine ──
    let mut blocks = state.cache.entry_blocks.clone();
    if !state.cache.streaming_blocks.is_empty() && !blocks.is_empty() {
        blocks.push(CachedBlock::spacing());
    }
    blocks.extend(state.cache.streaming_blocks.clone());

    // Working indicator
    if state.agent_running && state.streaming.as_ref().map_or(true, |s| s.blocks.is_empty()) {
        if !blocks.is_empty() { blocks.push(CachedBlock::spacing()); }
        blocks.push(CachedBlock {
            kind: BlockKind::Working,
            wrapped_height: 1,
            subagent_id: None,
        });
    }

    // ── 计算总高度 ──
    let wrapped_height = blocks.iter().map(|b| b.wrapped_height).sum();

    state.cache.blocks = blocks;
    state.cache.width = width;
    state.cache.wrapped_height = wrapped_height;
    state.cache.version = state.content_version;
    state.cache.last_rebuild = Some(Instant::now());
    state.cache_dirty = false;
}
```

### 3.2 辅助函数

```rust
fn entry_to_blocks(entry: &Entry, width: usize, out: &mut Vec<CachedBlock>) {
    match entry {
        Entry::System { text } => {
            let lines = system_lines(text, width);
            let h = lines_height(&lines, width);
            out.push(CachedBlock { kind: BlockKind::System(text.clone()), wrapped_height: h, subagent_id: None });
        }
        Entry::User { text } => {
            let lines = user_lines(text, width);
            let h = lines_height(&lines, width);
            out.push(CachedBlock { kind: BlockKind::User(text.clone()), wrapped_height: h, subagent_id: None });
        }
        Entry::Turn { blocks, .. } => {
            for block in blocks {
                turn_block_to_blocks(block, width, 0, out);
            }
        }
    }
}

fn turn_block_to_blocks(block: &TurnBlock, width: usize, indent: usize, out: &mut Vec<CachedBlock>) {
    // 类似当前的 render_turn_block_cloned，但生成 CachedBlock
}
```

**`lines_height`：** 对 `Vec<Line>` 的每行调用 `estimate_wrapped_rows` 并累加。

---

## 第四阶段：重写 render_conversation（render.rs）

```rust
fn render_conversation(frame: &mut Frame, state: &AppState, area: Rect) {
    let visible_height = area.height as usize;
    let total = state.cache.wrapped_height;
    let max_scroll = total.saturating_sub(visible_height);
    let scroll = state.scroll.min(max_scroll);
    let scroll_from_top = max_scroll.saturating_sub(scroll);

    // 找到第一个可见 block
    let mut offset = 0;
    let mut start_idx = 0;
    let mut start_skip = 0;
    for (i, block) in state.cache.blocks.iter().enumerate() {
        if offset + block.wrapped_height > scroll_from_top {
            start_idx = i;
            start_skip = scroll_from_top - offset;
            break;
        }
        offset += block.wrapped_height;
    }

    // 构建可见 block 约束
    let mut constraints = Vec::new();
    let mut visible_blocks = Vec::new();
    let mut remaining = visible_height;

    for block in state.cache.blocks.iter().skip(start_idx) {
        let h = (block.wrapped_height - start_skip).min(remaining);
        constraints.push(Constraint::Length(h as u16));
        visible_blocks.push((block, start_skip));
        remaining -= h;
        start_skip = 0;
        if remaining == 0 { break; }
    }

    let areas = Layout::vertical(constraints).split(area);

    // 渲染每个 block
    for (i, (block, skip)) in visible_blocks.iter().enumerate() {
        let block_area = areas[i];

        // Hover 高亮
        if let Some(ref hovered) = state.hovered_subagent {
            if block.subagent_id.as_ref() == Some(hovered) {
                buf.set_style(block_area, Style::default().bg(HOVER_BG));
            }
        }

        match &block.kind {
            BlockKind::User(text) => {
                frame.render_widget(UserBlock::new(text, *skip), block_area);
            }
            BlockKind::Thought(text) => {
                frame.render_widget(ThoughtBlock::new(text, *skip), block_area);
            }
            BlockKind::Response(text) => {
                frame.render_widget(ResponseBlock::new(text, *skip, area.width), block_area);
            }
            BlockKind::Tool { name, args, result } => {
                frame.render_widget(ToolBlock::new(name, args, result, state.frame_count, *skip), block_area);
            }
            BlockKind::Subagent(sa) => {
                frame.render_widget(SubagentBlock::new(sa, state.frame_count, *skip), block_area);
            }
            BlockKind::Notice(msg) => {
                frame.render_widget(NoticeBlock::new(msg, *skip), block_area);
            }
            BlockKind::Error(e) => {
                frame.render_widget(ErrorBlock::new(e, *skip), block_area);
            }
            BlockKind::System(text) => {
                frame.render_widget(SystemBlock::new(text, *skip), block_area);
            }
            BlockKind::Working => {
                frame.render_widget(WorkingBlock::new(state.frame_count), block_area);
            }
            BlockKind::Spacing => {}
        }
    }

    // Scrollbar（和原来一样）
    if max_scroll > 0 { ... }
}
```

---

## 第五阶段：简化鼠标处理（mod.rs）

`find_hovered_subagent` 不再需要 `row_offsets`：

```rust
fn find_hovered_subagent(mouse_y: u16, state: &AppState, main_area: Rect) -> Option<String> {
    if mouse_y < main_area.y || mouse_y >= main_area.y + main_area.height {
        return None;
    }
    let rel_y = (mouse_y - main_area.y) as usize;
    let visible_height = main_area.height as usize;
    let max_scroll = state.cache.wrapped_height.saturating_sub(visible_height);
    let scroll_from_top = max_scroll.saturating_sub(state.scroll);
    let abs_y = scroll_from_top + rel_y;

    let mut cumulative = 0;
    for block in &state.cache.blocks {
        let bottom = cumulative + block.wrapped_height;
        if abs_y >= cumulative && abs_y < bottom {
            return block.subagent_id.clone();
        }
        cumulative = bottom;
    }
    None
}
```

---

## 第六阶段：重写 render_subagent_detail

复用 `render_conversation` 的逻辑，只是 blocks 来源不同：

```rust
fn render_subagent_detail(frame: &mut Frame, state: &AppState, area: Rect) {
    // 1. 生成 subagent 的 blocks（和 rebuild_cache 中一样）
    // 2. 调用 render_blocks(frame, &blocks, state.subagent_scroll, area)
}
```

可以把 `render_conversation` 的核心逻辑提取为：

```rust
fn render_blocks(frame: &mut Frame, blocks: &[CachedBlock], scroll: usize, hovered: Option<&str>, frame_count: u64, area: Rect)
```

---

## 待删除的遗留代码

- `render.rs` 中的 `render_entry_cloned`, `render_turn_block_cloned`, `render_subagent_block`
- `render.rs` 中的 `animate_indicators`（移到各 Widget 内部）
- `render.rs` 中的 `compute_row_offsets`, `estimate_wrapped_rows`（保留但只用于 `rebuild_cache` 计算高度）
- `widgets/user_block.rs`（功能合并到 blocks.rs）
- `widgets/tool_block.rs`（功能合并到 blocks.rs）
- `state.rs` 中移除 `use ratatui::text::Line`

---

## 验收标准

1. `cargo check` 零 error
2. 代码中不再出现 `" ".repeat(` 用于背景色填充
3. `render()` 签名保持 `&AppState`
4. 滚动、hover、动画、diff 渲染功能正常
5. `render.rs` 行数控制在 400 行以内
