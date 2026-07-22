use super::state::{AppState, BlockKind, CachedBlock, ModalState};
use ratatui::{
    Frame,
    layout::{Constraint, Margin, Rect},
    style::{Color, Style},
    widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use std::time::Instant;

// ── Layout ──────────────────────────────────────────────────────────

pub struct LayoutAreas {
    pub status: Rect,
    pub main: Rect,
    pub dropdown: Rect,
    pub input: Rect,
}

/// Height of the input bar (bordered), growing with wrapped line count,
/// capped at 40% of the terminal height.
pub fn input_height(state: &AppState, width: u16) -> u16 {
    super::widgets::input_bar::estimate_height(&state.input, width)
}

pub fn compute_layout(area: Rect, input_h: u16) -> LayoutAreas {
    let input_h = input_h.max(3).min(area.height.saturating_sub(4).max(3));
    let status_h = 1u16;
    let input_top = area.height.saturating_sub(input_h);
    let main_bottom = input_top;
    let main_top = area.y + status_h;

    let status_area = Rect::new(area.x, area.y, area.width, status_h);
    let main_area = Rect::new(area.x, main_top, area.width, main_bottom.saturating_sub(main_top));
    let input_area = Rect::new(area.x, area.y + input_top, area.width, input_h);

    LayoutAreas {
        status: status_area,
        main: main_area,
        dropdown: Rect::default(),
        input: input_area,
    }
}

/// Autocomplete overlay rect — floats above the input bar, over the bottom
/// of the conversation area, without reflowing the main layout.
pub fn dropdown_overlay(state: &AppState, layout: &LayoutAreas) -> Option<Rect> {
    if !state.autocomplete.active {
        return None;
    }
    let h = (state.autocomplete.filtered.len().min(8) + 2) as u16;
    let y = layout.input.y.saturating_sub(h);
    Some(Rect::new(layout.input.x, y, layout.input.width, h))
}

pub fn render(frame: &mut Frame, state: &AppState) {
    let area = frame.area();
    let input_h = input_height(state, area.width);
    let layout = compute_layout(area, input_h);

    frame.render_widget(super::widgets::status::StatusBar::new(state), layout.status);

    if state.subagent_view.is_some() {
        render_subagent_detail(frame, state, layout.main);
    } else {
        render_conversation(frame, state, layout.main);
    }

    let input_bar = super::widgets::input_bar::InputBar::new(state);
    let cursor_pos = input_bar.cursor_position(layout.input);
    frame.render_widget(input_bar, layout.input);

    if let Some(overlay) = dropdown_overlay(state, &layout) {
        frame.render_widget(ratatui::widgets::Clear, overlay);
        frame.render_widget(super::widgets::dropdown::Dropdown::new(state), overlay);
    } else {
        frame.set_cursor_position(cursor_pos);
    }

    render_modal(frame, state, area);
}

fn render_modal(frame: &mut Frame, state: &AppState, area: Rect) {
    match &state.modal {
        ModalState::None => {}
        ModalState::ModelPicker { .. } | ModalState::ModelForm { .. } => {
            frame.render_widget(super::widgets::modal::Modal::new(state), area);
        }
        ModalState::Approval { .. } => {
            frame.render_widget(super::widgets::approval::ApprovalModal::new(state), area);
        }
        ModalState::Answer { .. } => {
            frame.render_widget(super::widgets::modal::AnswerModal::new(state), area);
        }
        ModalState::SessionList { .. } => {
            frame.render_widget(super::widgets::session_list::SessionListModal::new(state), area);
        }
        ModalState::Help => {
            frame.render_widget(super::widgets::help::HelpModal, area);
        }
        ModalState::Pager { .. } => {
            frame.render_widget(super::widgets::pager::PagerModal::new(state), area);
        }
        ModalState::RewindList { .. } => {
            frame.render_widget(super::widgets::modal::RewindModal::new(state), area);
        }
        ModalState::QuitConfirm => {
            frame.render_widget(super::widgets::modal::QuitConfirmModal, area);
        }
    }
}

// ── Conversation (block-level rendering) ─────────────────────────────

fn render_conversation(frame: &mut Frame, state: &AppState, area: Rect) {
    let visible_height = area.height as usize;
    let total = state.cache.wrapped_height;
    let max_scroll = total.saturating_sub(visible_height);
    let scroll = state.scroll.min(max_scroll);
    let scroll_from_top = max_scroll.saturating_sub(scroll);

    let hovered = state.hovered_subagent.as_deref();
    let content_area = area.inner(Margin { vertical: 0, horizontal: 1 });
    render_blocks(
        frame,
        &state.cache.blocks,
        state.frame_count,
        hovered,
        state.focused_block_id,
        scroll_from_top,
        content_area,
    );

    render_scrollbar(frame, area, max_scroll, scroll_from_top);
}

fn render_scrollbar(frame: &mut Frame, area: Rect, max_scroll: usize, position: usize) {
    if max_scroll == 0 {
        return;
    }
    let scrollbar_area = area.inner(Margin { vertical: 0, horizontal: 0 });
    let mut scrollbar_state = ScrollbarState::new(max_scroll).position(position);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .track_symbol(Some("│"))
            .end_symbol(Some("▼"))
            .thumb_style(Style::default().fg(Color::Rgb(92, 99, 112)))
            .track_style(Style::default().fg(Color::Rgb(40, 44, 52))),
        scrollbar_area,
        &mut scrollbar_state,
    );
}

/// Shared block rendering used by both main conversation and subagent detail.
fn render_blocks(
    frame: &mut Frame,
    blocks: &[CachedBlock],
    frame_count: u64,
    hovered_id: Option<&str>,
    focused_block_id: Option<u64>,
    scroll_from_top: usize,
    area: Rect,
) {
    use super::widgets::blocks as bw;

    let visible_height = area.height as usize;

    let mut offset = 0;
    let mut start_idx = 0;
    let mut start_skip = 0;
    for (i, block) in blocks.iter().enumerate() {
        if offset + block.wrapped_height > scroll_from_top {
            start_idx = i;
            start_skip = scroll_from_top.saturating_sub(offset);
            break;
        }
        offset += block.wrapped_height;
    }

    let mut constraints: Vec<Constraint> = Vec::new();
    let mut visible: Vec<(&CachedBlock, usize)> = Vec::new();
    let mut remaining = visible_height;

    for block in blocks.iter().skip(start_idx) {
        let h = (block.wrapped_height - start_skip).min(remaining);
        if h == 0 {
            break;
        }
        constraints.push(Constraint::Length(h as u16));
        visible.push((block, start_skip));
        remaining -= h;
        start_skip = 0;
        if remaining == 0 {
            break;
        }
    }

    if constraints.is_empty() {
        return;
    }

    let areas = ratatui::layout::Layout::vertical(constraints).split(area);

    for (i, (block, skip)) in visible.iter().enumerate() {
        let block_area = areas[i];
        let is_hovered = hovered_id.is_some_and(|id| block.subagent_id.as_deref() == Some(id));
        let is_focused = focused_block_id.is_some() && block.block_id == focused_block_id;

        match &block.kind {
            BlockKind::System(_) => {
                frame.render_widget(bw::SystemBlock::new(&block.lines, *skip), block_area);
            }
            BlockKind::Separator(label) => {
                frame.render_widget(bw::SeparatorBlock::new(label), block_area);
            }
            BlockKind::User(_) => {
                frame.render_widget(bw::UserBlock::new(&block.lines, *skip), block_area);
            }
            BlockKind::Thought { expanded, .. } => {
                frame.render_widget(
                    bw::ThoughtBlock::new(&block.lines, *skip, *expanded, is_focused),
                    block_area,
                );
            }
            BlockKind::Response(_) => {
                frame.render_widget(bw::ResponseBlock::new(&block.lines, *skip), block_area);
            }
            BlockKind::Tool { name, args, result, expanded, .. } => {
                frame.render_widget(
                    bw::ToolBlock::new(&block.lines, name, args, result, frame_count, *skip, *expanded, is_focused),
                    block_area,
                );
            }
            BlockKind::Subagent(sa) => {
                frame.render_widget(
                    bw::SubagentBlock::new(&block.lines, *skip, is_hovered, sa.done, sa.success),
                    block_area,
                );
            }
            BlockKind::Notice(_) => {
                frame.render_widget(bw::NoticeBlock::new(&block.lines, *skip), block_area);
            }
            BlockKind::Error(_) => {
                frame.render_widget(bw::ErrorBlock::new(&block.lines, *skip), block_area);
            }
            BlockKind::Working => {
                frame.render_widget(bw::WorkingBlock::new(frame_count), block_area);
            }
            BlockKind::Spacing => {}
        }
    }
}

// ── Cache rebuild ───────────────────────────────────────────────────

pub fn rebuild_cache(state: &mut AppState, width: u16) {
    use super::widgets::blocks;
    let w = width as usize;
    let entry_count = state.entries.len();

    let should_rebuild_entries =
        state.cache.rendered_entry_count != entry_count || state.cache.width != width || state.force_cache_rebuild;

    if should_rebuild_entries {
        let mut entry_blocks: Vec<CachedBlock> = Vec::new();
        let mut first = true;
        for entry in &state.entries {
            if !first {
                entry_blocks.push(CachedBlock::spacing());
            }
            first = false;
            blocks::entry_to_blocks(entry, &mut entry_blocks, w);
        }
        state.cache.entry_blocks = entry_blocks;
        state.cache.rendered_entry_count = entry_count;
    }

    let mut streaming_blocks: Vec<CachedBlock> = Vec::new();
    if let Some(ref streaming) = state.streaming {
        if !streaming.blocks.is_empty() {
            for (i, block) in streaming.blocks.iter().enumerate() {
                if i > 0 {
                    streaming_blocks.push(CachedBlock::spacing());
                }
                blocks::turn_block_to_blocks(block, &mut streaming_blocks, w, 0);
            }
        }
    }
    state.cache.streaming_blocks = streaming_blocks;

    let mut blocks_out: Vec<CachedBlock> = Vec::new();
    blocks_out.extend(state.cache.entry_blocks.iter().cloned());

    if !state.cache.streaming_blocks.is_empty() && !blocks_out.is_empty() {
        blocks_out.push(CachedBlock::spacing());
    }
    blocks_out.extend(state.cache.streaming_blocks.iter().cloned());

    let streaming_empty = state.streaming.as_ref().map_or(true, |s| s.blocks.is_empty());
    if state.agent_running && streaming_empty {
        if !blocks_out.is_empty() {
            blocks_out.push(CachedBlock::spacing());
        }
        let lines = blocks::working_block_lines();
        let height = blocks::compute_block_height(&lines, width);
        blocks_out.push(CachedBlock {
            kind: BlockKind::Working,
            wrapped_height: height,
            subagent_id: None,
            block_id: None,
            lines,
        });
    }

    let wrapped_height = blocks_out.iter().map(|b| b.wrapped_height).sum();
    let focusable_ids: Vec<u64> = blocks_out.iter().filter_map(|b| b.block_id).collect();

    state.cache.blocks = blocks_out;
    state.cache.width = width;
    state.cache.wrapped_height = wrapped_height;
    state.cache.version = state.content_version;
    state.cache.last_rebuild = Some(Instant::now());
    state.cache.focusable_ids = focusable_ids;
    state.cache_dirty = false;
    state.force_cache_rebuild = false;
}

// ── Subagent detail view ────────────────────────────────────────────

fn render_subagent_detail(frame: &mut Frame, state: &AppState, area: Rect) {
    use super::widgets::blocks;
    let width = area.width;
    let visible_height = area.height as usize;

    let subagent_id = match &state.subagent_view {
        Some(id) => id.clone(),
        None => return,
    };

    let sa = state.find_subagent(&subagent_id);
    let content_width = width.saturating_sub(2);
    let blks = blocks::subagent_detail_blocks(&subagent_id, sa, content_width);

    let wrapped_height: usize = blks.iter().map(|b| b.wrapped_height).sum();
    let max_scroll = wrapped_height.saturating_sub(visible_height);
    let subagent_scroll = state.subagent_scroll.min(max_scroll);
    let scroll_from_top = max_scroll.saturating_sub(subagent_scroll);

    let content_area = area.inner(Margin { vertical: 0, horizontal: 1 });
    render_blocks(frame, &blks, state.frame_count, None, None, scroll_from_top, content_area);
    render_scrollbar(frame, area, max_scroll, scroll_from_top);
}
