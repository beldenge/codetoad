use crate::agent::{ConfirmationDecision, ToolCallSummary};
use crate::confirmation::ConfirmationOperation;
use crate::tool_catalog::tool_display_name;
use crate::tools::ToolResult;
use anyhow::Result;
use crossterm::event::{self, Event as CEvent, KeyCode, KeyEventKind};
use crossterm::style::Stylize;
use std::io::{self, Write};

pub fn print_logo_and_tips() {
    for line in include_str!("../banner.txt").lines() {
        println!("{line}");
    }
    println!();
    println!("Tips for getting started:");
    println!("1. Ask questions, edit files, or run commands.");
    println!("2. Use /help for slash commands.");
    println!("3. Scrollback is native in inline mode (no alternate screen).");
    println!();
}

pub fn print_tool_result(call: ToolCallSummary, result: ToolResult) {
    println!(
        "{} {}",
        "●".magenta(),
        tool_label(&call).white()
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

pub fn tool_label(tool: &ToolCallSummary) -> String {
    format!("{}({})", pretty_tool_name(&tool.name), tool_target(tool))
}

pub fn prompt_tool_confirmation(
    tool_call: &ToolCallSummary,
    operation: ConfirmationOperation,
) -> Result<ConfirmationDecision> {
    println!();
    println!(
        "{} {}",
        "◦".yellow(),
        format!(
            "Confirmation required: {}({})",
            pretty_tool_name(&tool_call.name),
            tool_target(tool_call)
        )
        .yellow()
    );
    println!(
        "{}",
        format!("  operation: {}", confirmation_operation_label(operation)).dark_grey()
    );
    println!(
        "{}",
        format!("  details: {}", confirmation_detail(tool_call)).dark_grey()
    );
    println!(
        "{}",
        "  [y] approve once   [a] approve all for this session   [n]/[Esc] reject".dark_grey()
    );
    io::stdout().flush()?;

    loop {
        let event = event::read()?;
        let CEvent::Key(key) = event else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                println!("{}", "  -> approved".dark_green());
                return Ok(ConfirmationDecision::Approve {
                    tool_call_id: tool_call.id.clone(),
                    remember_for_session: false,
                });
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                println!(
                    "{}",
                    format!(
                        "  -> approved and remembered for {}",
                        confirmation_operation_label(operation)
                    )
                    .dark_green()
                );
                return Ok(ConfirmationDecision::Approve {
                    tool_call_id: tool_call.id.clone(),
                    remember_for_session: true,
                });
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                println!("{}", "  -> rejected".red());
                return Ok(ConfirmationDecision::Reject {
                    tool_call_id: tool_call.id.clone(),
                    feedback: None,
                });
            }
            _ => {}
        }
    }
}

fn confirmation_operation_label(operation: ConfirmationOperation) -> &'static str {
    match operation {
        ConfirmationOperation::File => "file operations",
        ConfirmationOperation::Bash => "bash commands",
    }
}

fn confirmation_detail(tool_call: &ToolCallSummary) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&tool_call.arguments) {
        if let Some(command) = value.get("command").and_then(serde_json::Value::as_str) {
            return format!("command: {command}");
        }
        if let Some(path) = value.get("path").and_then(serde_json::Value::as_str) {
            return format!("path: {path}");
        }
    }
    "operation details unavailable".to_string()
}

fn pretty_tool_name(name: &str) -> &str {
    tool_display_name(name)
}

fn tool_target(tool: &ToolCallSummary) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&tool.arguments) {
        return value
            .get("path")
            .and_then(serde_json::Value::as_str)
            .or_else(|| value.get("command").and_then(serde_json::Value::as_str))
            .or_else(|| value.get("query").and_then(serde_json::Value::as_str))
            .or_else(|| value.get("id").and_then(serde_json::Value::as_str))
            .unwrap_or("")
            .to_string();
    }
    String::new()
}
