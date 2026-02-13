use crate::protocol::ChatCompletionToolCallDelta;

#[derive(Debug, Clone)]
pub(crate) struct PartialToolCall {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) arguments: String,
}

pub(crate) fn accumulate_tool_calls(
    target: &mut Vec<PartialToolCall>,
    deltas: &[ChatCompletionToolCallDelta],
) {
    for delta in deltas {
        while target.len() <= delta.index {
            target.push(PartialToolCall {
                id: String::new(),
                name: String::new(),
                arguments: String::new(),
            });
        }

        let entry = &mut target[delta.index];
        if let Some(id) = &delta.id {
            entry.id.push_str(id);
        }
        if let Some(function) = &delta.function {
            if let Some(name) = &function.name {
                merge_stream_field(&mut entry.name, name);
            }
            if let Some(arguments) = &function.arguments {
                merge_stream_field(&mut entry.arguments, arguments);
            }
        }

        if entry.id.is_empty() {
            entry.id = format!("call_{}", delta.index);
        }
    }
}

pub(crate) fn merge_stream_text(target: &mut String, incoming: &str) -> Option<String> {
    if incoming.is_empty() {
        return None;
    }
    if target.is_empty() {
        target.push_str(incoming);
        return Some(incoming.to_string());
    }

    // Some streams send complete snapshots repeatedly instead of deltas.
    if incoming == target.as_str() {
        return None;
    }
    if incoming.starts_with(target.as_str()) {
        let suffix = &incoming[target.len()..];
        if suffix.is_empty() {
            return None;
        }
        target.push_str(suffix);
        return Some(suffix.to_string());
    }

    let appended = append_with_overlap(target, incoming);
    if appended.is_empty() {
        None
    } else {
        Some(appended)
    }
}

fn merge_stream_field(target: &mut String, delta: &str) {
    if delta.is_empty() {
        return;
    }
    if target.is_empty() {
        target.push_str(delta);
        return;
    }

    // Some providers emit full field values repeatedly instead of token deltas.
    // Replace with the longer prefix form rather than duplicating content.
    if delta.starts_with(target.as_str()) {
        *target = delta.to_string();
        return;
    }
    if target.as_str() == delta {
        return;
    }
    append_with_overlap(target, delta);
}

fn append_with_overlap(target: &mut String, incoming: &str) -> String {
    if incoming.is_empty() {
        return String::new();
    }

    let mut overlap_len = 0usize;
    let mut boundaries = Vec::new();
    boundaries.push(0usize);
    boundaries.extend(incoming.char_indices().map(|(idx, _)| idx).skip(1));
    boundaries.push(incoming.len());

    for size in boundaries.into_iter().rev() {
        if size == 0 || size > target.len() {
            continue;
        }
        if target.ends_with(&incoming[..size]) {
            overlap_len = size;
            break;
        }
    }

    let suffix = &incoming[overlap_len..];
    target.push_str(suffix);
    suffix.to_string()
}

#[cfg(test)]
mod tests {
    use super::{append_with_overlap, merge_stream_field, merge_stream_text};

    #[test]
    fn append_with_overlap_handles_suffix_overlap() {
        let mut target = "abcdef".to_string();
        let appended = append_with_overlap(&mut target, "defghi");
        assert_eq!(appended, "ghi");
        assert_eq!(target, "abcdefghi");
    }

    #[test]
    fn merge_stream_text_ignores_duplicate_full_snapshot() {
        let mut target = String::new();
        assert_eq!(
            merge_stream_text(&mut target, "hello"),
            Some("hello".to_string())
        );
        assert_eq!(merge_stream_text(&mut target, "hello"), None);
        assert_eq!(target, "hello");
    }

    #[test]
    fn merge_stream_text_emits_only_new_suffix_for_snapshots() {
        let mut target = String::new();
        assert_eq!(
            merge_stream_text(&mut target, "hello"),
            Some("hello".to_string())
        );
        assert_eq!(
            merge_stream_text(&mut target, "hello world"),
            Some(" world".to_string())
        );
        assert_eq!(target, "hello world");
    }

    #[test]
    fn merge_stream_field_handles_replayed_and_incremental_values() {
        let mut target = String::new();
        merge_stream_field(&mut target, "str_replace_editor");
        assert_eq!(target, "str_replace_editor");

        // Replayed full field should not duplicate content.
        merge_stream_field(&mut target, "str_replace_editor");
        assert_eq!(target, "str_replace_editor");

        // Incremental suffix appends correctly.
        merge_stream_field(&mut target, "_v2");
        assert_eq!(target, "str_replace_editor_v2");
    }
}
