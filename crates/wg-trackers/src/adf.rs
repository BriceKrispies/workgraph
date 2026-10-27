//! Atlassian Document Format flattening: the plain text of a Jira rich-text field.

use serde_json::Value;
use wg_types::pyval::text;

/// Block nodes: a node holding one of these as a child is flattened block by block.
const BLOCK_TYPES: &[&str] = &[
    "paragraph",
    "heading",
    "bulletList",
    "orderedList",
    "listItem",
    "codeBlock",
    "blockquote",
    "panel",
    "table",
    "tableRow",
    "tableHeader",
    "tableCell",
    "mediaSingle",
    "mediaGroup",
    "rule",
    "expand",
    "nestedExpand",
    "decisionList",
    "taskList",
];

/// Flattens an ADF tree to plain text.
///
/// Blocks are separated by blank lines, list items start with `- `, hard breaks become newlines,
/// mentions show their text and cards show their URL. Anything that is not an object is empty.
#[must_use]
pub fn adf_to_text(node: &Value) -> String {
    if !node.is_object() {
        return String::new();
    }
    blocks(node).join("\n\n").trim().to_string()
}

fn kind(node: &Value) -> Option<&str> {
    node.get("type").and_then(Value::as_str)
}

/// The object children of a node's `content`.
fn children(node: &Value) -> Vec<&Value> {
    node.get("content")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter(|item| item.is_object()).collect())
        .unwrap_or_default()
}

fn blocks(node: &Value) -> Vec<String> {
    let kids = children(node);
    if matches!(kind(node), Some("bulletList" | "orderedList")) {
        let items: Vec<String> = kids
            .iter()
            .map(|child| list_item(child))
            .filter(|item| !item.is_empty())
            .collect();
        return if items.is_empty() {
            Vec::new()
        } else {
            vec![items.join("\n")]
        };
    }
    if kids
        .iter()
        .any(|child| kind(child).is_some_and(|k| BLOCK_TYPES.contains(&k)))
    {
        return kids.iter().flat_map(|child| blocks(child)).collect();
    }
    let text = inline(node).trim().to_string();
    if text.is_empty() {
        Vec::new()
    } else {
        vec![text]
    }
}

fn list_item(node: &Value) -> String {
    let texts = blocks(node);
    if texts.is_empty() {
        String::new()
    } else {
        format!("- {}", texts.join("\n"))
    }
}

fn attr_text(node: &Value, name: &str) -> String {
    node.get("attrs")
        .and_then(|attrs| attrs.get(name))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn inline(node: &Value) -> String {
    match kind(node) {
        Some("text") => {
            let text = text(node.get("text"));
            if text.is_empty() {
                link_href(node)
            } else {
                text.to_string()
            }
        }
        Some("hardBreak") => "\n".to_string(),
        Some("mention" | "emoji" | "status") => attr_text(node, "text"),
        Some("inlineCard" | "blockCard" | "embedCard") => attr_text(node, "url"),
        _ => children(node).into_iter().map(inline).collect(),
    }
}

/// The `href` of the first link mark on a text node, or empty.
fn link_href(node: &Value) -> String {
    let Some(marks) = node.get("marks").and_then(Value::as_array) else {
        return String::new();
    };
    marks
        .iter()
        .find(|mark| mark.get("type").and_then(Value::as_str) == Some("link"))
        .and_then(|mark| mark.get("attrs"))
        .and_then(|attrs| attrs.get("href"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
