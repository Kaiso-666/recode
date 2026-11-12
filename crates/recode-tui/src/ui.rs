use crate::app::{App, Overlay};
use crate::theme::Theme;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use recode_schema::part::{Part, ToolStatus};
use unicode_width::UnicodeWidthStr;

pub fn draw(frame: &mut Frame, app: &App) {
    let theme = &app.theme;
    let area = frame.area();
    let input_visual = app.input.visual(area.width.saturating_sub(2) as usize);
    let input_height = (input_visual.0.len().clamp(1, 6) as u16) + 2;
    let todos = recode_tool::todos_for(&app.session.id);
    let todo_height = if todos.is_empty() {
        0
    } else {
        (todos.len().min(5) as u16) + 1
    };
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(todo_height),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .split(area);

    draw_header(frame, app, chunks[0]);
    draw_transcript(frame, app, chunks[1]);
    if todo_height > 0 {
        draw_todos(frame, &todos, chunks[2], theme);
    }
    draw_input(frame, app, chunks[3], input_visual);
    if app.overlay == Overlay::None {
        draw_completion(frame, app, chunks[3], area);
    }
    draw_footer(frame, app, chunks[4]);
    draw_overlay(frame, app, area);
}

fn draw_completion(frame: &mut Frame, app: &App, input_area: Rect, area: Rect) {
    let theme = &app.theme;
    let Some(completion) = &app.completion else {
        return;
    };
    if completion.items.is_empty() {
        return;
    }
    let height = (completion.items.len().min(8) as u16) + 2;
    let width = area.width.saturating_sub(8).min(100);
    let rect = Rect {
        x: area.x + 4,
        y: input_area.y.saturating_sub(height),
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.dim))
        .title(Span::styled(
            format!(" {}{} ", completion.sigil, completion.prefix),
            Style::default().fg(theme.dim),
        ));
    let inner = block.inner(rect);
    let items: Vec<ListItem> = completion
        .items
        .iter()
        .map(|path| {
            ListItem::new(Line::from(Span::styled(
                path.clone(),
                Style::default().fg(theme.text),
            )))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(theme.selected_bg)
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = ListState::default();
    state.select(Some(
        completion
            .index
            .min(completion.items.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(list, inner, &mut state);
    frame.render_widget(block, rect);
}

fn draw_todos(frame: &mut Frame, todos: &[recode_tool::TodoItem], area: Rect, theme: &Theme) {
    let width = area.width.saturating_sub(4) as usize;
    let mut lines = vec![Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "todos",
            Style::default().fg(theme.dim).add_modifier(Modifier::BOLD),
        ),
    ])];
    for todo in todos.iter().take(5) {
        let (marker, style) = match todo.status {
            recode_tool::TodoStatus::Completed => ("✓", Style::default().fg(theme.dim)),
            recode_tool::TodoStatus::InProgress => ("▶", Style::default().fg(theme.warn)),
            recode_tool::TodoStatus::Cancelled => ("×", Style::default().fg(theme.dim)),
            recode_tool::TodoStatus::Pending => ("·", Style::default().fg(theme.text)),
        };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{marker} "), style),
            Span::styled(truncate(&todo.content, width), style),
        ]));
    }
    if todos.len() > 5 {
        lines.push(Line::from(Span::styled(
            format!("  … {} more", todos.len() - 5),
            Style::default().fg(theme.dim),
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let title = truncate(
        &app.session.display_title(),
        area.width.saturating_sub(48) as usize,
    );
    let status = match app.status {
        recode_schema::event::SessionStatus::Idle => {
            Span::styled("idle", Style::default().fg(theme.dim))
        }
        recode_schema::event::SessionStatus::Busy => Span::styled(
            "working",
            Style::default().fg(theme.warn).add_modifier(Modifier::BOLD),
        ),
        recode_schema::event::SessionStatus::Retry => {
            Span::styled("retrying", Style::default().fg(theme.error))
        }
    };
    let line = Line::from(vec![
        Span::styled(
            " Re:Code ",
            Style::default()
                .fg(Color::Black)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            title,
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(theme.dim)),
        Span::styled(app.model_label(), Style::default().fg(theme.accent)),
        Span::styled("  ·  ", Style::default().fg(theme.dim)),
        Span::styled(app.agent_label(), Style::default().fg(theme.green)),
        Span::raw("  "),
        status,
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_transcript(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let width = area.width.saturating_sub(2) as usize;
    let mut lines = Vec::new();
    if app.messages.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "Welcome to Re:Code.",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "Describe a task. I can read the codebase, edit files, and run commands.",
                Style::default().fg(theme.dim),
            ),
        ]));
        lines.push(Line::from(""));
        for hint in [
            "  Enter        send message",
            "  Alt+Enter    new line",
            "  Esc          abort the current run",
            "  F1           help",
            "  F2 / F3      choose model / agent",
            "  F4           resume a session",
        ] {
            lines.push(Line::from(Span::styled(
                hint,
                Style::default().fg(theme.dim),
            )));
        }
    }
    for message in &app.messages {
        let (label, color) = match message.info.role {
            recode_schema::Role::User => ("you", theme.green),
            recode_schema::Role::Assistant => ("recode", theme.accent),
        };
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("▌ ", Style::default().fg(color)),
            Span::styled(
                label,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ]));
        for part in &message.parts {
            match part {
                Part::Text(text) => {
                    if text.synthetic {
                        push_text(
                            &mut lines,
                            &text.text,
                            width,
                            2,
                            Style::default().fg(theme.dim),
                            true,
                        );
                    } else {
                        push_markdown(&mut lines, &text.text, width, 2, theme);
                    }
                }
                Part::Reasoning(reasoning) => {
                    if app.show_reasoning && !reasoning.text.trim().is_empty() {
                        lines.push(Line::from(vec![
                            Span::raw("  "),
                            Span::styled(
                                "· thinking",
                                Style::default()
                                    .fg(theme.dim)
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ]));
                        let text = limit_lines(&reasoning.text, 40);
                        push_text(
                            &mut lines,
                            &text,
                            width,
                            4,
                            Style::default()
                                .fg(theme.dim)
                                .add_modifier(Modifier::ITALIC),
                            false,
                        );
                    }
                }
                Part::Tool(tool) => draw_tool(&mut lines, tool, width, app.tick, theme),
                Part::StepFinish(finish) => {
                    let usage = &finish.tokens;
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            format!(
                                "· step · {} in / {} out · ${:.4}",
                                format_tokens(usage.input_tokens),
                                format_tokens(usage.output_tokens),
                                finish.cost
                            ),
                            Style::default().fg(theme.dim),
                        ),
                    ]));
                }
                Part::Error(error) => {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            "error: ",
                            Style::default()
                                .fg(theme.error)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(error.message.clone(), Style::default().fg(theme.error)),
                    ]));
                }
                Part::File(file) => {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            format!("[{}] {}", file.mime, file.filename),
                            Style::default().fg(theme.dim),
                        ),
                    ]));
                }
                Part::Compaction(_) => {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled("· context compacted", Style::default().fg(theme.dim)),
                    ]));
                }
                Part::Agent(agent) => {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            format!("· agent: {}", agent.name),
                            Style::default().fg(theme.dim),
                        ),
                    ]));
                }
                Part::Model(model) => {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            format!("· model: {}", model.model.qualified()),
                            Style::default().fg(theme.dim),
                        ),
                    ]));
                }
                Part::Patch(patch) => {
                    if !patch.files.is_empty() {
                        let mut preview = patch
                            .files
                            .iter()
                            .take(3)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ");
                        if patch.files.len() > 3 {
                            preview.push_str(&format!(" +{}", patch.files.len() - 3));
                        }
                        lines.push(Line::from(vec![
                            Span::raw("  "),
                            Span::styled(
                                format!("· changed {} file(s): {preview}", patch.files.len()),
                                Style::default().fg(theme.dim),
                            ),
                        ]));
                    }
                }
                Part::StepStart(_) | Part::Snapshot(_) => {}
            }
        }
    }

    let total = lines.len();
    let viewport = area.height as usize;
    let max_scroll = total.saturating_sub(viewport);
    let back = app.scroll_back.min(max_scroll);
    let scroll = max_scroll - back;
    let paragraph = Paragraph::new(lines).scroll((scroll as u16, 0));
    frame.render_widget(paragraph, area);

    if back > 0 && total > viewport {
        let indicator = Rect {
            x: area.x + area.width.saturating_sub(12),
            y: area.y + area.height.saturating_sub(1),
            width: 12,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "▼ scrolled up ",
                Style::default().fg(theme.warn),
            ))),
            indicator,
        );
    }
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect, visual: (Vec<String>, usize, usize)) {
    let theme = &app.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(
            if app.status == recode_schema::event::SessionStatus::Busy {
                theme.warn
            } else {
                theme.accent
            },
        ))
        .title(Span::styled(" message ", Style::default().fg(theme.dim)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (lines, cursor_row, cursor_col) = visual;
    let visible = inner.height as usize;
    let offset = lines.len().saturating_sub(visible);
    let displayed: Vec<Line> = if app.input.is_empty() {
        vec![Line::from(Span::styled(
            "Ask Re:Code to build something…   (F1 for help)",
            Style::default().fg(theme.dim),
        ))]
    } else {
        lines
            .iter()
            .skip(offset)
            .map(|line| Line::from(Span::styled(line.clone(), Style::default().fg(theme.text))))
            .collect()
    };
    frame.render_widget(Paragraph::new(displayed), inner);
    if app.overlay == Overlay::None {
        let row = cursor_row.saturating_sub(offset) as u16;
        let x = inner.x + (cursor_col as u16).min(inner.width.saturating_sub(1));
        let y = inner.y + row.min(inner.height.saturating_sub(1));
        frame.set_cursor_position((x, y));
    }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let usage = app.usage();
    let right = if usage.total_tokens > 0 {
        format!(
            "{} tok · ${:.4} ",
            format_tokens(usage.total_tokens),
            usage.cost
        )
    } else {
        String::new()
    };
    let right_width = right.width() as u16;
    let chunks =
        Layout::horizontal([Constraint::Min(10), Constraint::Length(right_width)]).split(area);
    if let Some((notice, _)) = &app.notice {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {notice}"),
                Style::default().fg(theme.warn),
            ))),
            chunks[0],
        );
    } else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " Enter send · Alt+Enter newline · Esc abort · F1 help · F2 model · F3 agent · F4 sessions",
                Style::default().fg(theme.dim),
            ))),
            chunks[0],
        );
    }
    if right_width > 0 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                right,
                Style::default().fg(theme.dim),
            ))),
            chunks[1],
        );
    }
}

fn draw_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    if app.overlay == Overlay::None {
        return;
    }
    let rect = centered(area, 78, 64);
    frame.render_widget(Clear, rect);
    let title = match app.overlay {
        Overlay::Help => " help ".to_owned(),
        Overlay::Model => " model ".to_owned(),
        Overlay::Agent => " agent ".to_owned(),
        Overlay::Sessions if !app.picker_filter.is_empty() => {
            format!(" sessions [{}] ", app.picker_filter)
        }
        Overlay::Sessions => " sessions ".to_owned(),
        Overlay::Permission => " permission ".to_owned(),
        Overlay::Question => " question ".to_owned(),
        Overlay::None => String::new(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .title(Span::styled(
            title,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    match app.overlay {
        Overlay::Help => {
            let mut lines: Vec<Line> = [
                "",
                "  Enter            send the message",
                "  Alt+Enter / ^J   insert a newline",
                "  Esc              abort the current run",
                "  PageUp/PageDown  scroll the transcript",
                "  F2               choose a model",
                "  F3               choose an agent",
                "  F4               resume another session",
                "  Ctrl+R           show/hide reasoning",
                "  Ctrl+C           abort, or quit when idle",
                "",
                "  Slash commands   /new /fork /undo /model /agent /sessions /compact /abort /reasoning /theme /clear /quit",
                "",
                "  @path            complete a project file (↑/↓, Enter/Tab)",
                "  F4 + typing      search sessions by title or id",
            ]
            .iter()
            .map(|line| Line::from(Span::styled(*line, Style::default().fg(theme.text))))
            .collect();
            let custom: Vec<String> = app
                .backend
                .commands()
                .into_iter()
                .map(|command| format!("/{}", command.name))
                .collect();
            if !custom.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("  Custom commands  {}", custom.join(" ")),
                    Style::default().fg(theme.text),
                )));
            }
            frame.render_widget(Paragraph::new(lines), inner);
        }
        Overlay::Model | Overlay::Agent | Overlay::Sessions => {
            let visible = app.visible_picker();
            let items: Vec<ListItem> = visible
                .iter()
                .map(|item| {
                    ListItem::new(Line::from(Span::styled(
                        item.label.clone(),
                        Style::default().fg(theme.text),
                    )))
                })
                .collect();
            let list = List::new(items)
                .highlight_style(
                    Style::default()
                        .bg(theme.selected_bg)
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("› ");
            let mut state = ListState::default();
            state.select(if visible.is_empty() {
                None
            } else {
                Some(app.picker_index.min(visible.len() - 1))
            });
            frame.render_stateful_widget(list, inner, &mut state);
        }
        Overlay::Permission => {
            if let Some(request) = app.current_permission() {
                let mut lines = vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        format!("tool: {}", request.tool.clone().unwrap_or_default()),
                        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                ];
                for pattern in &request.patterns {
                    for line in wrap_text(pattern, inner.width.saturating_sub(4) as usize) {
                        lines.push(Line::from(Span::styled(
                            format!("  {line}"),
                            Style::default().fg(theme.warn),
                        )));
                    }
                }
                if let Some(input) = request.metadata.get("input") {
                    let text = serde_json::to_string_pretty(input).unwrap_or_default();
                    for line in wrap_text(&text, inner.width.saturating_sub(4) as usize)
                        .into_iter()
                        .take(10)
                    {
                        lines.push(Line::from(Span::styled(
                            format!("  {line}"),
                            Style::default().fg(theme.dim),
                        )));
                    }
                }
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled(
                        "  [y]",
                        Style::default()
                            .fg(theme.green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" allow once    ", Style::default().fg(theme.text)),
                    Span::styled(
                        "[a]",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" always    ", Style::default().fg(theme.text)),
                    Span::styled(
                        "[n]",
                        Style::default()
                            .fg(theme.error)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" reject", Style::default().fg(theme.text)),
                ]));
                frame.render_widget(Paragraph::new(lines), inner);
            }
        }
        Overlay::Question => {
            if let Some((request, progress)) = app.current_question() {
                if let Some(question) = request.questions.get(progress.index) {
                    let width = inner.width.saturating_sub(4) as usize;
                    let mut lines = vec![
                        Line::from(""),
                        Line::from(Span::styled(
                            format!(
                                "question {} of {}",
                                progress.index + 1,
                                request.questions.len()
                            ),
                            Style::default().fg(theme.dim),
                        )),
                        Line::from(""),
                    ];
                    if !question.header.is_empty() {
                        lines.push(Line::from(Span::styled(
                            format!("  {}", question.header),
                            Style::default()
                                .fg(theme.accent)
                                .add_modifier(Modifier::BOLD),
                        )));
                    }
                    for line in wrap_text(&question.question, width) {
                        lines.push(Line::from(Span::styled(
                            format!("  {line}"),
                            Style::default().fg(theme.text),
                        )));
                    }
                    lines.push(Line::from(""));
                    for (index, option) in question.options.iter().enumerate() {
                        let active = index == progress.cursor;
                        let chosen =
                            progress.selected.contains(&index) || (!question.multiple && active);
                        let style = if active {
                            Style::default()
                                .fg(theme.accent)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(theme.text)
                        };
                        lines.push(Line::from(vec![
                            Span::raw(if active { "  › " } else { "    " }),
                            Span::styled(
                                if chosen { "[x] " } else { "[ ] " },
                                Style::default().fg(if chosen { theme.green } else { theme.dim }),
                            ),
                            Span::styled(option.label.clone(), style),
                        ]));
                        if let Some(description) = &option.description {
                            if !description.is_empty() {
                                for line in wrap_text(description, width.saturating_sub(8))
                                    .into_iter()
                                    .take(3)
                                {
                                    lines.push(Line::from(Span::styled(
                                        format!("        {line}"),
                                        Style::default().fg(theme.dim),
                                    )));
                                }
                            }
                        }
                    }
                    lines.push(Line::from(""));
                    lines.push(Line::from(vec![
                        Span::styled("  ↑/↓", Style::default().fg(theme.accent)),
                        Span::styled(" choose   ", Style::default().fg(theme.dim)),
                        Span::styled("space", Style::default().fg(theme.accent)),
                        Span::styled(" select   ", Style::default().fg(theme.dim)),
                        Span::styled("enter", Style::default().fg(theme.green)),
                        Span::styled(" confirm   ", Style::default().fg(theme.dim)),
                        Span::styled("esc", Style::default().fg(theme.error)),
                        Span::styled(" decline", Style::default().fg(theme.dim)),
                    ]));
                    frame.render_widget(Paragraph::new(lines), inner);
                }
            }
        }
        Overlay::None => {}
    }
}

fn draw_tool(
    lines: &mut Vec<Line<'static>>,
    tool: &recode_schema::part::ToolPart,
    width: usize,
    tick: u64,
    theme: &Theme,
) {
    let (icon, color) = match tool.state.status {
        ToolStatus::Pending => ("○", theme.dim),
        ToolStatus::Running => (["◐", "◓", "◑", "◒"][(tick / 2 % 4) as usize], theme.warn),
        ToolStatus::Completed => ("●", theme.tool),
        ToolStatus::Error => ("✗", theme.error),
    };
    let title = tool
        .state
        .title
        .clone()
        .unwrap_or_else(|| tool.tool.clone());
    let duration = tool
        .state
        .time
        .as_ref()
        .and_then(|time| time.end.map(|end| end - time.start))
        .filter(|duration| *duration >= 0)
        .map(|duration| format!("  {:.1}s", duration as f64 / 1000.0))
        .unwrap_or_default();
    let header = format!("{icon} {}  {title}{duration}", tool.tool);
    for (index, line) in wrap_text(&header, width.saturating_sub(2))
        .into_iter()
        .enumerate()
    {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                if index == 0 {
                    line
                } else {
                    format!("  {line}")
                },
                Style::default().fg(color),
            ),
        ]));
    }
    let body = match tool.state.status {
        ToolStatus::Completed => tool.state.output.as_deref().unwrap_or(""),
        ToolStatus::Error => tool.state.error.as_deref().unwrap_or(""),
        _ => "",
    };
    if !body.trim().is_empty() {
        let style = if tool.state.status == ToolStatus::Error {
            Style::default().fg(theme.error)
        } else {
            Style::default().fg(theme.dim)
        };
        let preview = limit_lines(body, 6);
        push_text(lines, &preview, width, 4, style, true);
    }
}

fn push_markdown(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    indent: usize,
    theme: &Theme,
) {
    let mut in_code = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            let mut styled = Vec::new();
            push_text(
                &mut styled,
                &format!("│ {line}"),
                width,
                indent,
                Style::default().fg(theme.text).bg(theme.code_bg),
                false,
            );
            lines.extend(styled);
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix('#') {
            let heading = heading.trim_start_matches('#').trim();
            push_text(
                lines,
                heading,
                width,
                indent,
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
                true,
            );
            continue;
        }
        let cleaned = if let Some(rest) = line.strip_prefix("- ") {
            format!("• {rest}")
        } else {
            strip_inline_markup(line)
        };
        push_text(
            lines,
            &cleaned,
            width,
            indent,
            Style::default().fg(theme.text),
            true,
        );
    }
}

fn strip_inline_markup(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '`' => {}
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
            }
            _ => output.push(character),
        }
    }
    output
}

fn push_text(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    indent: usize,
    style: Style,
    wrap: bool,
) {
    let prefix = " ".repeat(indent);
    if text.is_empty() {
        lines.push(Line::from(Span::raw(prefix)));
        return;
    }
    for line in text.split('\n') {
        if wrap {
            for wrapped in wrap_text(line, width.saturating_sub(indent)) {
                lines.push(Line::from(Span::styled(
                    format!("{prefix}{wrapped}"),
                    style,
                )));
            }
        } else {
            lines.push(Line::from(Span::styled(format!("{prefix}{line}"), style)));
        }
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    if text.is_empty() {
        return vec![String::new()];
    }
    if text.width() <= width {
        return vec![text.to_owned()];
    }
    let mut output = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for word in text.split(' ') {
        let word_width = word.width();
        if current_width > 0 && current_width + 1 + word_width > width {
            output.push(std::mem::take(&mut current));
            current_width = 0;
        }
        if word_width > width {
            if !current.is_empty() {
                output.push(std::mem::take(&mut current));
                current_width = 0;
            }
            let mut chunk = String::new();
            let mut chunk_width = 0;
            for character in word.chars() {
                let character_width = character.to_string().width();
                if chunk_width + character_width > width {
                    output.push(std::mem::take(&mut chunk));
                    chunk_width = 0;
                }
                chunk.push(character);
                chunk_width += character_width;
            }
            if !chunk.is_empty() {
                current = chunk;
                current_width = chunk_width;
            }
            continue;
        }
        if current_width > 0 {
            current.push(' ');
            current_width += 1;
        }
        current.push_str(word);
        current_width += word_width;
    }
    if !current.is_empty() || output.is_empty() {
        output.push(current);
    }
    output
}

fn limit_lines(text: &str, limit: usize) -> String {
    let lines: Vec<&str> = text.lines().take(limit).collect();
    let mut output = lines.join("\n");
    if text.lines().count() > limit {
        output.push_str("\n…");
    }
    output
}

fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

fn truncate(text: &str, width: usize) -> String {
    let width = width.max(8);
    if text.width() <= width {
        return text.to_owned();
    }
    let mut output = String::new();
    let mut used = 0;
    for character in text.chars() {
        let character_width = character.to_string().width();
        if used + character_width + 1 > width {
            output.push('…');
            break;
        }
        output.push(character);
        used += character_width;
    }
    output
}

fn centered(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);
    let horizontal = Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(vertical[1]);
    horizontal[1]
}
