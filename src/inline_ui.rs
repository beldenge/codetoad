use crate::agent::{Agent, AgentEvent, ToolCallSummary};
use crate::settings::SettingsManager;
use crate::tools::{ToolResult, execute_bash_command};
use anyhow::Result;
use crossterm::cursor::{MoveDown, MoveLeft, MoveToColumn, MoveUp};
use crossterm::event::{self, DisableMouseCapture, Event as CEvent, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::style::Stylize;
use crossterm::terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode};
use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, mpsc};
use tokio::time::{self, Duration};
use tokio_util::sync::CancellationToken;

const DIRECT_COMMANDS: &[&str] = &[
    "ls", "pwd", "cd", "cat", "mkdir", "touch", "echo", "grep", "find", "cp", "mv", "rm",
];
const COMMAND_SUGGESTIONS: &[(&str, &str)] = &[
    ("/help", "Show help information"),
    ("/clear", "Clear chat history"),
    ("/models", "Switch Grok model"),
    ("/commit-and-push", "AI commit and push"),
    ("/exit", "Exit application"),
];
const STATUS_FRAMES: &[&str] = &["-", "\\", "|", "/"];
const STREAM_FLUSH_THRESHOLD: usize = 16;

#[derive(Default)]
struct MarkdownStreamRenderer {
    pending: String,
    flushed_prefix: usize,
    in_code_block: bool,
    code_lang: String,
}

pub async fn run_inline(
    agent: Arc<Mutex<Agent>>,
    settings: Arc<Mutex<SettingsManager>>,
    initial_message: Option<String>,
) -> Result<()> {
    recover_terminal_state();
    print_logo_and_tips();
    let mut history: Vec<String> = Vec::new();

    if let Some(initial) = initial_message {
        history.push(initial.clone());
        handle_input(&initial, agent.clone(), settings.clone()).await?;
    }

    loop {
        let Some(input) = read_prompt_line(&history)? else {
            break;
        };
        let input = input.trim().to_string();
        if input.is_empty() {
            continue;
        }
        if input == "exit" || input == "quit" || input == "/exit" {
            break;
        }
        history.push(input.clone());
        handle_input(&input, agent.clone(), settings.clone()).await?;
    }

    Ok(())
}

fn recover_terminal_state() {
    let _ = disable_raw_mode();
    let mut stdout = io::stdout();
    let _ = execute!(stdout, DisableMouseCapture);
}

async fn handle_input(
    input: &str,
    agent: Arc<Mutex<Agent>>,
    settings: Arc<Mutex<SettingsManager>>,
) -> Result<()> {
    if input == "/help" {
        println!("{}", help_text());
        return Ok(());
    }

    if input == "/clear" {
        agent.lock().await.reset_conversation();
        clear_screen();
        print_logo_and_tips();
        return Ok(());
    }

    if input == "/models" {
        let available = settings.lock().await.get_available_models();
        let current = agent.lock().await.current_model().to_string();
        match select_model_inline(&available, &current)? {
            Some(model) => {
                agent.lock().await.set_model(model.clone());
                settings.lock().await.update_project_model(&model)?;
                println!("Switched to model: {model}");
            }
            None => {
                println!("Model selection cancelled.");
            }
        }
        return Ok(());
    }

    if let Some(model) = input.strip_prefix("/models ").map(str::trim) {
        let available = settings.lock().await.get_available_models();
        if available.iter().any(|m| m == model) {
            agent.lock().await.set_model(model.to_string());
            settings.lock().await.update_project_model(model)?;
            println!("Switched to model: {model}");
        } else {
            println!("Invalid model: {model}");
            println!("Available: {}", available.join(", "));
        }
        return Ok(());
    }

    if input == "/commit-and-push" {
        run_commit_and_push(agent).await?;
        return Ok(());
    }

    if is_direct_command(input) {
        let result = execute_bash_command(input).await?;
        print_tool_result(
            ToolCallSummary {
                id: "bash_inline".to_string(),
                name: "bash".to_string(),
                arguments: format!(r#"{{"command":"{}"}}"#, input.replace('"', "\\\"")),
            },
            result,
        );
        return Ok(());
    }

    stream_agent_message(input.to_string(), agent).await
}

async fn stream_agent_message(message: String, agent: Arc<Mutex<Agent>>) -> Result<()> {
    let cancel_token = CancellationToken::new();
    let (agent_tx, mut agent_rx) = mpsc::unbounded_channel::<AgentEvent>();

    let error_tx = agent_tx.clone();
    let agent_for_task = agent.clone();
    tokio::spawn(async move {
        let result = agent_for_task
            .lock()
            .await
            .process_user_message_stream(message, cancel_token, agent_tx)
            .await;
        if let Err(err) = result {
            error_tx
                .send(AgentEvent::Error(format!("{err:#}")))
                .ok();
            error_tx.send(AgentEvent::Done).ok();
        }
    });

    let mut started_content = false;
    let mut phase = "thinking";
    let mut tool_calls_seen = 0usize;
    let mut tool_results_seen = 0usize;
    let mut frame_idx = 0usize;
    let mut status_width = 0usize;
    let started_at = Instant::now();
    let mut status_tick = time::interval(Duration::from_millis(120));
    let mut renderer = MarkdownStreamRenderer::default();
    let mut tool_started_at: HashMap<String, Instant> = HashMap::new();
    let mut tool_succeeded = 0usize;
    let mut tool_failed = 0usize;

    loop {
        let event = tokio::select! {
            _ = status_tick.tick(), if !started_content => {
                let elapsed = started_at.elapsed().as_secs();
                let progress = if phase == "running tools" && tool_calls_seen > 0 {
                    format!(
                        " ({}/{})",
                        tool_results_seen.min(tool_calls_seen),
                        tool_calls_seen
                    )
                } else {
                    String::new()
                };
                let status = format!(
                    "{} {}{}... {}s",
                    STATUS_FRAMES[frame_idx % STATUS_FRAMES.len()],
                    phase,
                    progress,
                    elapsed
                );
                frame_idx = frame_idx.wrapping_add(1);
                render_status_line(&status, &mut status_width)?;
                continue;
            }
            maybe_event = agent_rx.recv() => maybe_event,
        };

        let Some(event) = event else {
            clear_status_line(&mut status_width)?;
            if started_content {
                flush_markdown_pending(&mut renderer)?;
                println!();
            }
            break;
        };

        match event {
            AgentEvent::Content(chunk) => {
                if !started_content {
                    clear_status_line(&mut status_width)?;
                    print!("{} ", "●".white());
                    started_content = true;
                }
                stream_markdown_chunk(&mut renderer, &chunk)?;
            }
            AgentEvent::ToolCalls(calls) => {
                if started_content {
                    flush_markdown_pending(&mut renderer)?;
                    println!();
                    started_content = false;
                }
                phase = "running tools";
                tool_calls_seen += calls.len();
                clear_status_line(&mut status_width)?;
                for call in calls {
                    let label = format!("{}({})", pretty_tool_name(&call.name), tool_target(&call));
                    println!(
                        "{} {}",
                        "◦".magenta(),
                        format!("start {label}").dark_grey()
                    );
                    println!(
                        "{} {}",
                        "●".magenta(),
                        label.white()
                    );
                    println!("{}", "  -> Executing...".cyan());
                    tool_started_at.insert(call.id.clone(), Instant::now());
                }
            }
            AgentEvent::ToolResult { tool_call, result } => {
                if started_content {
                    flush_markdown_pending(&mut renderer)?;
                    println!();
                    started_content = false;
                }
                tool_results_seen = tool_results_seen.saturating_add(1);
                phase = if tool_calls_seen == 0 {
                    "running tools"
                } else if tool_results_seen >= tool_calls_seen {
                    "finalizing"
                } else {
                    "running tools"
                };
                clear_status_line(&mut status_width)?;
                let label = format!(
                    "{}({})",
                    pretty_tool_name(&tool_call.name),
                    tool_target(&tool_call)
                );
                let elapsed = tool_started_at
                    .remove(&tool_call.id)
                    .map(|start| format_elapsed(start.elapsed()))
                    .unwrap_or_else(|| "n/a".to_string());
                if result.success {
                    tool_succeeded = tool_succeeded.saturating_add(1);
                    println!(
                        "{} {}",
                        "◦".magenta(),
                        format!("done {label} in {elapsed}").dark_green()
                    );
                } else {
                    tool_failed = tool_failed.saturating_add(1);
                    println!(
                        "{} {}",
                        "◦".magenta(),
                        format!("failed {label} in {elapsed}").red()
                    );
                }
                print_tool_result(tool_call, result);
            }
            AgentEvent::Done => {
                clear_status_line(&mut status_width)?;
                if started_content {
                    flush_markdown_pending(&mut renderer)?;
                    println!();
                }
                if tool_calls_seen > 0 {
                    println!(
                        "{}",
                        format!(
                            "◦ tools summary: {} total, {} succeeded, {} failed",
                            tool_calls_seen, tool_succeeded, tool_failed
                        )
                        .dark_grey()
                    );
                }
                let elapsed = started_at.elapsed();
                println!(
                    "{}",
                    format!(
                        "● completed in {}.{:01}s",
                        elapsed.as_secs(),
                        elapsed.subsec_millis() / 100
                    )
                    .dark_grey()
                );
                break;
            }
            AgentEvent::Error(err) => {
                clear_status_line(&mut status_width)?;
                if started_content {
                    flush_markdown_pending(&mut renderer)?;
                    println!();
                }
                if tool_calls_seen > 0 {
                    println!(
                        "{}",
                        format!(
                            "◦ tools summary: {} total, {} succeeded, {} failed",
                            tool_calls_seen, tool_succeeded, tool_failed
                        )
                        .dark_grey()
                    );
                }
                println!("{}", format!("Error: {err}").red());
                break;
            }
        }
    }

    Ok(())
}

async fn run_commit_and_push(agent: Arc<Mutex<Agent>>) -> Result<()> {
    println!("Running commit-and-push...");

    let status = execute_bash_command("git status --porcelain").await?;
    if !status.success
        || status
            .output
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
    {
        println!("No changes to commit.");
        return Ok(());
    }

    let add = execute_bash_command("git add .").await?;
    print_tool_result(
        ToolCallSummary {
            id: "git_add_inline".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command":"git add ."}"#.to_string(),
        },
        add,
    );

    let diff = execute_bash_command("git diff --cached")
        .await
        .ok()
        .and_then(|r| r.output)
        .unwrap_or_default();
    let prompt = format!(
        "Generate a concise conventional commit message under 72 characters.\n\nGit Status:\n{}\n\nGit Diff:\n{}\n\nRespond with only the commit message.",
        status.output.unwrap_or_default(),
        diff
    );

    let message = match agent.lock().await.generate_plain_text(&prompt).await {
        Ok(text) if !text.trim().is_empty() => text.trim().trim_matches('"').to_string(),
        _ => "chore: update project files".to_string(),
    };
    println!("Generated commit message: \"{message}\"");

    let commit_cmd = format!("git commit -m \"{}\"", message.replace('"', "\\\""));
    let commit = execute_bash_command(&commit_cmd).await?;
    let commit_success = commit.success;
    print_tool_result(
        ToolCallSummary {
            id: "git_commit_inline".to_string(),
            name: "bash".to_string(),
            arguments: format!(r#"{{"command":"{}"}}"#, commit_cmd.replace('"', "\\\"")),
        },
        commit,
    );

    if commit_success {
        let mut push_cmd = "git push".to_string();
        let mut push = execute_bash_command(&push_cmd).await?;
        if !push.success
            && push
                .error
                .as_deref()
                .map(|e| e.contains("no upstream branch"))
                .unwrap_or(false)
        {
            push_cmd = "git push -u origin HEAD".to_string();
            push = execute_bash_command(&push_cmd).await?;
        }

        print_tool_result(
            ToolCallSummary {
                id: "git_push_inline".to_string(),
                name: "bash".to_string(),
                arguments: format!(r#"{{"command":"{}"}}"#, push_cmd.replace('"', "\\\"")),
            },
            push,
        );
    }

    Ok(())
}

fn print_logo_and_tips() {
    let logo = [
        "  /\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\",
        " /\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/",
        " \\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\",
        "  /\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\",
    ];
    for line in logo {
        println!("{line}");
    }
    println!();
    println!("Tips for getting started:");
    println!("1. Ask questions, edit files, or run commands.");
    println!("2. Use /help for slash commands.");
    println!("3. Scrollback is native in inline mode (no alternate screen).");
    println!();
}

fn print_tool_result(call: ToolCallSummary, result: ToolResult) {
    println!(
        "{} {}",
        "●".magenta(),
        format!("{}({})", pretty_tool_name(&call.name), tool_target(&call)).white()
    );
    if result.success {
        if let Some(output) = result.output {
            for line in output.replace("\r\n", "\n").split('\n') {
                println!("{}", format!("  -> {line}").dark_grey());
            }
        } else {
            println!("{}", "  -> Success".dark_grey());
        }
    } else if let Some(error) = result.error {
        for line in error.replace("\r\n", "\n").split('\n') {
            println!("{}", format!("  -> {line}").red());
        }
    } else {
        println!("{}", "  -> Error".red());
    }
}

fn pretty_tool_name(name: &str) -> &str {
    match name {
        "view_file" => "Read",
        "str_replace_editor" => "Update",
        "create_file" => "Create",
        "bash" => "Bash",
        _ => "Tool",
    }
}

fn tool_target(tool: &ToolCallSummary) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&tool.arguments) {
        return value
            .get("path")
            .and_then(serde_json::Value::as_str)
            .or_else(|| value.get("command").and_then(serde_json::Value::as_str))
            .unwrap_or("")
            .to_string();
    }
    String::new()
}

fn is_direct_command(input: &str) -> bool {
    let first = input.split_whitespace().next().unwrap_or_default();
    DIRECT_COMMANDS.contains(&first)
}

fn help_text() -> &'static str {
    "Grok Build Help:\n\nBuilt-in Commands:\n  /clear            Clear chat history\n  /help             Show this help\n  /models           Switch between available models\n  /models <name>    Set model directly\n  /exit             Exit application\n\nGit Commands:\n  /commit-and-push  AI-generated commit and push\n\nDirect Commands:\n  ls, pwd, cd, cat, mkdir, touch, echo, grep, find, cp, mv, rm\n\nInput Controls:\n  Up/Down       History (or command suggestion selection)\n  Left/Right    Move cursor\n  Tab           Accept command suggestion\n  Enter         Submit input (or accept suggestion when / command hints are visible)\n  Ctrl+A/E      Start/end of line\n  Ctrl+U/W      Delete to start / delete previous word\n  Ctrl+C        Clear input (press twice on empty input to exit)\n\nInline mode keeps native terminal scrollback, shows live elapsed status while working, and preserves output after Ctrl+C."
}

fn clear_screen() {
    print!("\x1b[2J\x1b[H");
    let _ = io::stdout().flush();
}

fn render_status_line(text: &str, prev_width: &mut usize) -> io::Result<()> {
    let width = text.chars().count();
    let padding = prev_width.saturating_sub(width);
    print!("\r{}{}", text.dark_grey(), " ".repeat(padding));
    io::stdout().flush()?;
    *prev_width = width;
    Ok(())
}

fn clear_status_line(prev_width: &mut usize) -> io::Result<()> {
    if *prev_width > 0 {
        print!("\r{}\r", " ".repeat(*prev_width));
        io::stdout().flush()?;
        *prev_width = 0;
    }
    Ok(())
}

fn format_elapsed(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    let tenths = elapsed.subsec_millis() / 100;
    format!("{secs}.{tenths}s")
}

fn stream_markdown_chunk(renderer: &mut MarkdownStreamRenderer, chunk: &str) -> io::Result<()> {
    renderer.pending.push_str(chunk);

    while let Some(newline_idx) = renderer.pending.find('\n') {
        let line = renderer.pending[..newline_idx].to_string();
        let already_printed = renderer.flushed_prefix.min(line.len());
        if already_printed > 0 {
            let remainder = &line[already_printed..];
            print!("{remainder}");
        } else {
            render_markdown_line(renderer, &line)?;
        }
        println!();
        renderer.pending = renderer.pending[(newline_idx + 1)..].to_string();
        renderer.flushed_prefix = 0;
    }

    let unflushed = renderer.pending.len().saturating_sub(renderer.flushed_prefix);
    if unflushed >= STREAM_FLUSH_THRESHOLD {
        let delta = &renderer.pending[renderer.flushed_prefix..];
        print!("{delta}");
        renderer.flushed_prefix = renderer.pending.len();
    }

    io::stdout().flush()
}

fn flush_markdown_pending(renderer: &mut MarkdownStreamRenderer) -> io::Result<()> {
    if renderer.pending.is_empty() {
        return Ok(());
    }

    let already_printed = renderer.flushed_prefix.min(renderer.pending.len());
    if already_printed > 0 {
        let remainder = &renderer.pending[already_printed..];
        print!("{remainder}");
    } else {
        let line = renderer.pending.clone();
        render_markdown_line(renderer, &line)?;
    }

    renderer.pending.clear();
    renderer.flushed_prefix = 0;
    io::stdout().flush()
}

fn render_markdown_line(renderer: &mut MarkdownStreamRenderer, line: &str) -> io::Result<()> {
    let trimmed = line.trim_start();

    if trimmed.starts_with("```") {
        if renderer.in_code_block {
            renderer.in_code_block = false;
            renderer.code_lang.clear();
        } else {
            renderer.in_code_block = true;
            renderer.code_lang = trimmed
                .trim_start_matches("```")
                .trim()
                .to_lowercase();
        }
        print!("{}", line.dark_grey());
        return Ok(());
    }

    if renderer.in_code_block {
        render_code_line(line, &renderer.code_lang)?;
        return Ok(());
    }

    if is_heading_line(trimmed) {
        print!("{}", line.cyan().bold());
        return Ok(());
    }

    if trimmed.starts_with("> ") {
        print!("{}", line.dark_grey());
        return Ok(());
    }

    if let Some((indent, marker, rest)) = split_list_prefix(line) {
        print!("{indent}");
        print!("{}", marker.cyan());
        render_inline_markdown(rest)?;
        return Ok(());
    }

    render_inline_markdown(line)
}

fn render_inline_markdown(line: &str) -> io::Result<()> {
    let mut in_code = false;
    let mut buf = String::new();

    for ch in line.chars() {
        if ch == '`' {
            if !buf.is_empty() {
                if in_code {
                    print!("{}", buf.as_str().yellow());
                } else {
                    print!("{buf}");
                }
                buf.clear();
            }
            in_code = !in_code;
            continue;
        }
        buf.push(ch);
    }

    if !buf.is_empty() {
        if in_code {
            print!("{}", buf.as_str().yellow());
        } else {
            print!("{buf}");
        }
    }

    io::stdout().flush()
}

fn render_code_line(line: &str, lang: &str) -> io::Result<()> {
    let mut chars = line.chars().peekable();
    let mut in_string: Option<char> = None;
    let mut string_buf = String::new();
    let mut word_buf = String::new();
    let comment_prefix = code_comment_prefix(lang);

    while let Some(ch) = chars.next() {
        if let Some(quote) = in_string {
            string_buf.push(ch);
            let escaped = string_buf
                .chars()
                .rev()
                .nth(1)
                .map(|c| c == '\\')
                .unwrap_or(false);
            if ch == quote && !escaped {
                print!("{}", string_buf.as_str().yellow());
                string_buf.clear();
                in_string = None;
            }
            continue;
        }

        if is_comment_start(ch, chars.peek().copied(), comment_prefix) {
            flush_code_word(&word_buf, lang);
            word_buf.clear();
            let mut comment = ch.to_string();
            if let Some(next) = chars.peek().copied()
                && ((comment_prefix == "//" && next == '/') || (comment_prefix == "--" && next == '-'))
            {
                comment.push(chars.next().unwrap_or_default());
            }
            for c in chars {
                comment.push(c);
            }
            print!("{}", comment.dark_green());
            io::stdout().flush()?;
            return Ok(());
        }

        if ch == '"' || ch == '\'' {
            flush_code_word(&word_buf, lang);
            word_buf.clear();
            in_string = Some(ch);
            string_buf.push(ch);
            continue;
        }

        if ch.is_alphanumeric() || ch == '_' {
            word_buf.push(ch);
            continue;
        }

        flush_code_word(&word_buf, lang);
        word_buf.clear();
        print!("{}", ch.to_string().dark_cyan());
    }

    flush_code_word(&word_buf, lang);
    if !string_buf.is_empty() {
        print!("{}", string_buf.dark_yellow());
    }
    io::stdout().flush()
}

fn flush_code_word(word: &str, lang: &str) {
    if word.is_empty() {
        return;
    }
    if word.chars().all(|ch| ch.is_ascii_digit()) {
        print!("{}", word.dark_yellow());
    } else if is_lang_keyword(lang, word) {
        print!("{}", word.cyan().bold());
    } else {
        print!("{}", word.dark_cyan());
    }
}

fn code_comment_prefix(lang: &str) -> &'static str {
    match lang {
        "python" | "py" | "bash" | "sh" | "zsh" | "yaml" | "yml" | "toml" => "#",
        "sql" => "--",
        _ => "//",
    }
}

fn is_comment_start(current: char, next: Option<char>, prefix: &str) -> bool {
    match prefix {
        "#" => current == '#',
        "--" => current == '-' && next == Some('-'),
        _ => current == '/' && next == Some('/'),
    }
}

fn is_heading_line(trimmed: &str) -> bool {
    let level = trimmed.chars().take_while(|ch| *ch == '#').count();
    level > 0 && level <= 6 && trimmed.chars().nth(level) == Some(' ')
}

fn split_list_prefix(line: &str) -> Option<(&str, &str, &str)> {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let trimmed = &line[indent_len..];

    if let Some(rest) = trimmed.strip_prefix("- ") {
        return Some((indent, "- ", rest));
    }
    if let Some(rest) = trimmed.strip_prefix("* ") {
        return Some((indent, "* ", rest));
    }

    let mut chars = trimmed.chars().peekable();
    let mut digit_count = 0usize;
    while matches!(chars.peek(), Some(ch) if ch.is_ascii_digit()) {
        chars.next();
        digit_count += 1;
    }
    if digit_count == 0 {
        return None;
    }
    if chars.next() != Some('.') || chars.next() != Some(' ') {
        return None;
    }

    let marker_len = digit_count + 2;
    let marker = &trimmed[..marker_len];
    let rest = &trimmed[marker_len..];
    Some((indent, marker, rest))
}

fn is_lang_keyword(lang: &str, word: &str) -> bool {
    match lang {
        "rust" | "rs" => matches!(
            word,
            "fn"
                | "let"
                | "mut"
                | "pub"
                | "struct"
                | "enum"
                | "impl"
                | "trait"
                | "match"
                | "if"
                | "else"
                | "for"
                | "while"
                | "loop"
                | "return"
                | "use"
                | "mod"
                | "async"
                | "await"
                | "where"
                | "const"
                | "static"
        ),
        "typescript" | "ts" | "javascript" | "js" | "tsx" | "jsx" => matches!(
            word,
            "function"
                | "const"
                | "let"
                | "var"
                | "return"
                | "if"
                | "else"
                | "for"
                | "while"
                | "class"
                | "import"
                | "export"
                | "from"
                | "async"
                | "await"
                | "try"
                | "catch"
                | "throw"
                | "new"
                | "interface"
                | "type"
        ),
        "python" | "py" => matches!(
            word,
            "def"
                | "class"
                | "return"
                | "if"
                | "elif"
                | "else"
                | "for"
                | "while"
                | "import"
                | "from"
                | "try"
                | "except"
                | "finally"
                | "with"
                | "as"
                | "async"
                | "await"
                | "lambda"
        ),
        "bash" | "sh" | "zsh" => matches!(
            word,
            "if" | "then" | "else" | "fi" | "for" | "do" | "done" | "case" | "esac" | "function"
        ),
        "json" => matches!(word, "true" | "false" | "null"),
        _ => false,
    }
}

fn read_prompt_line(history: &[String]) -> Result<Option<String>> {
    enable_raw_mode()?;
    let mut input = String::new();
    let mut cursor = 0usize;
    let mut history_idx: Option<usize> = None;
    let mut ctrl_c_armed = false;
    let mut selected_suggestion_idx = 0usize;
    let mut suggestions_visible = false;
    render_prompt_with_suggestions(
        &input,
        cursor,
        build_command_hint(&input, selected_suggestion_idx).as_deref(),
        &mut suggestions_visible,
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
                    input = format!("{} ", suggestions[safe].0);
                    cursor = input.len();
                    ctrl_c_armed = false;
                    history_idx = None;
                    selected_suggestion_idx = 0;
                    let hint = build_command_hint(&input, selected_suggestion_idx);
                    render_prompt_with_suggestions(
                        &input,
                        cursor,
                        hint.as_deref(),
                        &mut suggestions_visible,
                    )?;
                    continue;
                }
                clear_suggestion_line(&mut suggestions_visible)?;
                disable_raw_mode()?;
                print!("\r\n");
                io::stdout().flush()?;
                return Ok(Some(input));
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if input.is_empty() {
                    if ctrl_c_armed {
                        clear_suggestion_line(&mut suggestions_visible)?;
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
                    clear_suggestion_line(&mut suggestions_visible)?;
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
                    input = format!("{} ", suggestions[safe].0);
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

        let hint = build_command_hint(&input, selected_suggestion_idx);
        render_prompt_with_suggestions(
            &input,
            cursor,
            hint.as_deref(),
            &mut suggestions_visible,
        )?;
    }
}

fn render_prompt_with_suggestions(
    input: &str,
    cursor: usize,
    suggestion: Option<&str>,
    suggestions_visible: &mut bool,
) -> io::Result<()> {
    execute!(io::stdout(), MoveToColumn(0), Clear(ClearType::CurrentLine))?;
    print!("{} {}", ">".cyan(), input);
    execute!(io::stdout(), Clear(ClearType::UntilNewLine))?;

    if let Some(text) = suggestion {
        execute!(io::stdout(), MoveDown(1), MoveToColumn(0), Clear(ClearType::CurrentLine))?;
        print!("{}", text.dark_grey());
        execute!(io::stdout(), MoveUp(1))?;
        *suggestions_visible = true;
    } else if *suggestions_visible {
        execute!(io::stdout(), MoveDown(1), MoveToColumn(0), Clear(ClearType::CurrentLine), MoveUp(1))?;
        *suggestions_visible = false;
    }

    let tail = input[cursor..].chars().count();
    if tail > 0 {
        execute!(io::stdout(), MoveLeft(tail as u16))?;
    }
    io::stdout().flush()
}

fn clear_suggestion_line(suggestions_visible: &mut bool) -> io::Result<()> {
    if *suggestions_visible {
        execute!(io::stdout(), MoveDown(1), MoveToColumn(0), Clear(ClearType::CurrentLine), MoveUp(1))?;
        *suggestions_visible = false;
    }
    Ok(())
}

fn select_model_inline(available: &[String], current: &str) -> Result<Option<String>> {
    if available.is_empty() {
        println!("No models available.");
        return Ok(None);
    }

    let mut selected = available.iter().position(|m| m == current).unwrap_or(0);
    enable_raw_mode()?;
    let mut rendered_lines = 0usize;

    loop {
        if rendered_lines > 0 {
            execute!(io::stdout(), MoveUp(rendered_lines as u16), MoveToColumn(0))?;
            for _ in 0..rendered_lines {
                execute!(io::stdout(), Clear(ClearType::CurrentLine), MoveDown(1), MoveToColumn(0))?;
            }
            execute!(io::stdout(), MoveUp(rendered_lines as u16), MoveToColumn(0))?;
        }

        println!("Select model (↑/↓ navigate, Enter/Tab confirm, Esc cancel):");
        for (idx, model) in available.iter().enumerate() {
            let marker = if idx == selected { ">" } else { " " };
            let current_suffix = if model == current { " (current)" } else { "" };
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

fn filtered_command_suggestions(input: &str) -> Vec<(&'static str, &'static str)> {
    if !input.starts_with('/') {
        return Vec::new();
    }

    let prefix = input.trim();
    COMMAND_SUGGESTIONS
        .iter()
        .copied()
        .filter(|(cmd, _)| cmd.starts_with(prefix))
        .collect()
}

fn build_command_hint(input: &str, selected_index: usize) -> Option<String> {
    if !input.starts_with('/') {
        return None;
    }

    let matches = filtered_command_suggestions(input);
    if matches.is_empty() {
        return Some("commands: (no matches)".to_string());
    }

    let safe = selected_index.min(matches.len().saturating_sub(1));
    let mut parts = Vec::new();
    for (idx, (cmd, _)) in matches.iter().take(5).enumerate() {
        let rendered = if idx == safe {
            format!("[{cmd}]")
        } else {
            (*cmd).to_string()
        };
        parts.push(rendered);
    }
    if matches.len() > 5 {
        parts.push("...".to_string());
    }

    Some(format!(
        "commands: {}    selected: {} - {}",
        parts.join("  "),
        matches[safe].0,
        matches[safe].1
    ))
}
