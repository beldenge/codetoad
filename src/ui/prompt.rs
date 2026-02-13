use crate::slash_commands::filtered_command_suggestions;
use anyhow::Result;
use crossterm::cursor::{MoveDown, MoveToColumn, MoveUp};
use crossterm::event::{self, Event as CEvent, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::style::Stylize;
use crossterm::terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode, size};
use std::io::{self, Write};

pub fn read_prompt_line(
    history: &[String],
    auto_edit: &mut bool,
    current_model: &str,
) -> Result<Option<String>> {
    enable_raw_mode()?;
    let mut input = String::new();
    let mut cursor = 0usize;
    let mut history_idx: Option<usize> = None;
    let mut ctrl_c_armed = false;
    let mut selected_suggestion_idx = 0usize;
    let mut rendered_panel_lines = 0usize;
    rerender_prompt_input(
        &input,
        cursor,
        selected_suggestion_idx,
        *auto_edit,
        current_model,
        &mut rendered_panel_lines,
    )?;

    loop {
        let event = event::read()?;
        let CEvent::Key(key) = event else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::Enter => {
                let suggestions = filtered_command_suggestions(&input);
                if !suggestions.is_empty() {
                    let safe = selected_suggestion_idx.min(suggestions.len().saturating_sub(1));
                    let selected = suggestions[safe].command;
                    let trimmed = input.trim();
                    if trimmed != selected {
                        input = format!("{selected} ");
                        cursor = input.len();
                        ctrl_c_armed = false;
                        history_idx = None;
                        selected_suggestion_idx = 0;
                        rerender_prompt_input(
                            &input,
                            cursor,
                            selected_suggestion_idx,
                            *auto_edit,
                            current_model,
                            &mut rendered_panel_lines,
                        )?;
                        continue;
                    }
                }
                clear_prompt_panel(&mut rendered_panel_lines)?;
                disable_raw_mode()?;
                print!("\r\n");
                io::stdout().flush()?;
                return Ok(Some(input));
            }
            KeyCode::BackTab => {
                *auto_edit = !*auto_edit;
                ctrl_c_armed = false;
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if input.is_empty() {
                    if ctrl_c_armed {
                        clear_prompt_panel(&mut rendered_panel_lines)?;
                        disable_raw_mode()?;
                        print!("\r\n");
                        io::stdout().flush()?;
                        return Ok(None);
                    }
                    ctrl_c_armed = true;
                    print!("\r\x1b[2K{}\r\n", "Press Ctrl+C again to exit.".dark_grey());
                } else {
                    input.clear();
                    cursor = 0;
                    history_idx = None;
                    ctrl_c_armed = false;
                    selected_suggestion_idx = 0;
                }
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if input.is_empty() {
                    clear_prompt_panel(&mut rendered_panel_lines)?;
                    disable_raw_mode()?;
                    print!("\r\n");
                    io::stdout().flush()?;
                    return Ok(None);
                }
                if cursor < input.len() {
                    let next = next_boundary(&input, cursor);
                    input.drain(cursor..next);
                    ctrl_c_armed = false;
                    selected_suggestion_idx = 0;
                }
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                cursor = 0;
                ctrl_c_armed = false;
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                cursor = input.len();
                ctrl_c_armed = false;
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                input.drain(..cursor);
                cursor = 0;
                history_idx = None;
                ctrl_c_armed = false;
                selected_suggestion_idx = 0;
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let start = previous_word_start(&input, cursor);
                input.drain(start..cursor);
                cursor = start;
                history_idx = None;
                ctrl_c_armed = false;
                selected_suggestion_idx = 0;
            }
            KeyCode::Left => {
                cursor = prev_boundary(&input, cursor);
                ctrl_c_armed = false;
            }
            KeyCode::Right => {
                cursor = next_boundary(&input, cursor);
                ctrl_c_armed = false;
            }
            KeyCode::Home => {
                cursor = 0;
                ctrl_c_armed = false;
            }
            KeyCode::End => {
                cursor = input.len();
                ctrl_c_armed = false;
            }
            KeyCode::Backspace => {
                if cursor > 0 {
                    let prev = prev_boundary(&input, cursor);
                    input.drain(prev..cursor);
                    cursor = prev;
                    history_idx = None;
                    selected_suggestion_idx = 0;
                }
                ctrl_c_armed = false;
            }
            KeyCode::Delete => {
                if cursor < input.len() {
                    let next = next_boundary(&input, cursor);
                    input.drain(cursor..next);
                    history_idx = None;
                    selected_suggestion_idx = 0;
                }
                ctrl_c_armed = false;
            }
            KeyCode::Up => {
                let suggestions = filtered_command_suggestions(&input);
                if !suggestions.is_empty() {
                    selected_suggestion_idx = if selected_suggestion_idx == 0 {
                        suggestions.len().saturating_sub(1)
                    } else {
                        selected_suggestion_idx.saturating_sub(1)
                    };
                } else if !history.is_empty() {
                    let next = history_idx
                        .map(|idx| idx.saturating_sub(1))
                        .unwrap_or_else(|| history.len().saturating_sub(1));
                    history_idx = Some(next);
                    input = history[next].clone();
                    cursor = input.len();
                    selected_suggestion_idx = 0;
                }
                ctrl_c_armed = false;
            }
            KeyCode::Down => {
                let suggestions = filtered_command_suggestions(&input);
                if !suggestions.is_empty() {
                    selected_suggestion_idx = (selected_suggestion_idx + 1) % suggestions.len();
                } else if !history.is_empty() {
                    match history_idx {
                        None => {}
                        Some(idx) if idx + 1 >= history.len() => {
                            history_idx = None;
                            input.clear();
                            cursor = 0;
                        }
                        Some(idx) => {
                            let next = idx + 1;
                            history_idx = Some(next);
                            input = history[next].clone();
                            cursor = input.len();
                        }
                    }
                    selected_suggestion_idx = 0;
                }
                ctrl_c_armed = false;
            }
            KeyCode::Tab => {
                let suggestions = filtered_command_suggestions(&input);
                if !suggestions.is_empty() {
                    let safe = selected_suggestion_idx.min(suggestions.len().saturating_sub(1));
                    input = format!("{} ", suggestions[safe].command);
                    cursor = input.len();
                    history_idx = None;
                    selected_suggestion_idx = 0;
                }
                ctrl_c_armed = false;
            }
            KeyCode::Char(ch) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    input.insert(cursor, ch);
                    cursor += ch.len_utf8();
                    history_idx = None;
                    ctrl_c_armed = false;
                    selected_suggestion_idx = 0;
                }
            }
            _ => {
                ctrl_c_armed = false;
            }
        }

        rerender_prompt_input(
            &input,
            cursor,
            selected_suggestion_idx,
            *auto_edit,
            current_model,
            &mut rendered_panel_lines,
        )?;
    }
}

pub fn select_model_inline(available: &[String], current: &str) -> Result<Option<String>> {
    select_option_inline(
        "Select model",
        available,
        Some(current),
        "No models available.",
    )
}

pub fn select_option_inline(
    title: &str,
    available: &[String],
    current: Option<&str>,
    empty_message: &str,
) -> Result<Option<String>> {
    if available.is_empty() {
        println!("{empty_message}");
        return Ok(None);
    }

    let mut selected = current
        .and_then(|current| available.iter().position(|value| value == current))
        .unwrap_or(0);
    enable_raw_mode()?;
    let mut rendered_lines = 0usize;

    loop {
        if rendered_lines > 0 {
            execute!(io::stdout(), MoveUp(rendered_lines as u16), MoveToColumn(0))?;
            for _ in 0..rendered_lines {
                execute!(
                    io::stdout(),
                    Clear(ClearType::CurrentLine),
                    MoveDown(1),
                    MoveToColumn(0)
                )?;
            }
            execute!(io::stdout(), MoveUp(rendered_lines as u16), MoveToColumn(0))?;
        }

        println!("{title} (↑/↓ navigate, Enter/Tab confirm, Esc cancel):");
        for (idx, model) in available.iter().enumerate() {
            let marker = if idx == selected { ">" } else { " " };
            let current_suffix = if Some(model.as_str()) == current {
                " (current)"
            } else {
                ""
            };
            println!("{marker} {model}{current_suffix}");
        }
        io::stdout().flush()?;
        rendered_lines = available.len() + 1;

        let event = event::read()?;
        let CEvent::Key(key) = event else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::Up => {
                selected = if selected == 0 {
                    available.len().saturating_sub(1)
                } else {
                    selected.saturating_sub(1)
                };
            }
            KeyCode::Down => {
                selected = (selected + 1) % available.len();
            }
            KeyCode::Enter | KeyCode::Tab => {
                disable_raw_mode()?;
                execute!(io::stdout(), MoveToColumn(0))?;
                return Ok(Some(available[selected].clone()));
            }
            KeyCode::Esc => {
                disable_raw_mode()?;
                execute!(io::stdout(), MoveToColumn(0))?;
                return Ok(None);
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                disable_raw_mode()?;
                execute!(io::stdout(), MoveToColumn(0))?;
                return Ok(None);
            }
            _ => {}
        }
    }
}

fn rerender_prompt_input(
    input: &str,
    cursor: usize,
    selected_suggestion_idx: usize,
    auto_edit: bool,
    current_model: &str,
    rendered_panel_lines: &mut usize,
) -> io::Result<()> {
    let panel = build_prompt_panel(input, selected_suggestion_idx, auto_edit, current_model);
    render_prompt_with_suggestions(input, cursor, &panel, rendered_panel_lines)
}

fn render_prompt_with_suggestions(
    input: &str,
    cursor: usize,
    panel_lines: &[String],
    rendered_panel_lines: &mut usize,
) -> io::Result<()> {
    execute!(io::stdout(), MoveToColumn(0), Clear(ClearType::CurrentLine))?;
    print!("{} {}", ">".cyan(), input);
    execute!(io::stdout(), Clear(ClearType::UntilNewLine))?;

    clear_prompt_panel(rendered_panel_lines)?;

    if !panel_lines.is_empty() {
        execute!(io::stdout(), MoveDown(1), MoveToColumn(0))?;
        for (idx, line) in panel_lines.iter().enumerate() {
            execute!(io::stdout(), Clear(ClearType::CurrentLine))?;
            print!("{}", fit_terminal_line(line).dark_grey());
            if idx + 1 < panel_lines.len() {
                execute!(io::stdout(), MoveDown(1), MoveToColumn(0))?;
            }
        }
        execute!(
            io::stdout(),
            MoveUp(panel_lines.len() as u16),
            MoveToColumn(0)
        )?;
        *rendered_panel_lines = panel_lines.len();
    }

    let prompt_prefix_cols = 2usize; // "> "
    let input_cursor_cols = input[..cursor].chars().count();
    let terminal_cols = size().map(|(cols, _)| cols as usize).unwrap_or(120usize);
    let max_col = terminal_cols.saturating_sub(1);
    let target_col = (prompt_prefix_cols + input_cursor_cols).min(max_col);
    execute!(io::stdout(), MoveToColumn(target_col as u16))?;
    io::stdout().flush()
}

fn clear_prompt_panel(rendered_panel_lines: &mut usize) -> io::Result<()> {
    if *rendered_panel_lines > 0 {
        execute!(io::stdout(), MoveDown(1), MoveToColumn(0))?;
        for idx in 0..*rendered_panel_lines {
            execute!(io::stdout(), Clear(ClearType::CurrentLine))?;
            if idx + 1 < *rendered_panel_lines {
                execute!(io::stdout(), MoveDown(1), MoveToColumn(0))?;
            }
        }
        execute!(
            io::stdout(),
            MoveUp(*rendered_panel_lines as u16),
            MoveToColumn(0)
        )?;
        *rendered_panel_lines = 0;
    }
    Ok(())
}

fn fit_terminal_line(text: &str) -> String {
    let width = size().map(|(cols, _)| cols as usize).unwrap_or(120usize);
    let max = width.saturating_sub(1);
    if max == 0 {
        return String::new();
    }

    let len = text.chars().count();
    if len <= max {
        return text.to_string();
    }
    if max == 1 {
        return "…".to_string();
    }

    let kept = max - 1;
    let mut clipped = text.chars().take(kept).collect::<String>();
    clipped.push('…');
    clipped
}

fn prev_boundary(input: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    input[..cursor]
        .char_indices()
        .last()
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

fn next_boundary(input: &str, cursor: usize) -> usize {
    if cursor >= input.len() {
        return input.len();
    }
    let mut iter = input[cursor..].char_indices();
    let _ = iter.next();
    if let Some((offset, _)) = iter.next() {
        cursor + offset
    } else {
        input.len()
    }
}

fn previous_word_start(input: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let mut index = cursor;

    while index > 0 {
        let prev = prev_boundary(input, index);
        if !input[prev..index]
            .chars()
            .next()
            .map(|ch| ch.is_whitespace())
            .unwrap_or(false)
        {
            break;
        }
        index = prev;
    }

    while index > 0 {
        let prev = prev_boundary(input, index);
        if input[prev..index]
            .chars()
            .next()
            .map(|ch| ch.is_whitespace())
            .unwrap_or(false)
        {
            break;
        }
        index = prev;
    }

    index
}

fn build_prompt_panel(
    input: &str,
    selected_index: usize,
    auto_edit: bool,
    current_model: &str,
) -> Vec<String> {
    let status = format!(
        "{} auto-edit: {} (shift + tab)   ~= {}",
        if auto_edit { "▶" } else { "⏸" },
        if auto_edit { "on" } else { "off" },
        current_model
    );
    let mut lines = vec![status];

    if !input.starts_with('/') {
        return lines;
    }

    let matches = filtered_command_suggestions(input);
    if matches.is_empty() {
        lines.push("slash commands: (no matches)".to_string());
        return lines;
    }

    lines.push("slash commands:".to_string());
    let safe = selected_index.min(matches.len().saturating_sub(1));
    let display_limit = 6usize;
    for (idx, command) in matches.iter().take(display_limit).enumerate() {
        let marker = if idx == safe { ">" } else { " " };
        lines.push(format!(
            "  {marker} {:<18} {}",
            command.command, command.description
        ));
    }
    if matches.len() > display_limit {
        lines.push(format!("    ... +{} more", matches.len() - display_limit));
    }
    lines.push("    ↑/↓ navigate  Tab autocomplete  Enter run".to_string());

    lines
}
