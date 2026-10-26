//! The tool, resource and resource-template descriptors.
//!
//! `contracts/mcp.json` lists the descriptors sorted by name, with snake_case keys. The Python
//! server answers `tools/list` in registration order and with camelCase keys, so the order is
//! kept in `tool_order.txt` (checked against the recorded `tools/list` reply by the tests) and the
//! keys are renamed here.
//!
//! The snapshot also sorts every `properties` object by name, which loses the order of the
//! Python function signature (and of the nested models), and that order is the order Pydantic
//! reports a multi-error message in. `argument_order.json` keeps it; the arguments model is
//! loaded from a copy of the schema whose `properties` follow it, while the descriptor served on
//! the wire stays the snapshot's.

use serde_json::{Map, Value, json};
use wg_types::{ErrorCode, WgError};
use wg_validate::{LoadOptions, Model, model_from_json_schema};

/// The tool names in registration order.
const TOOL_ORDER: &str = include_str!("tool_order.txt");

/// The declaration order of each tool's arguments and of the nested models they use, from the
/// Python signatures (`{"tools": {name: [argument, ...]}, "defs": {model: [field, ...]}}`).
const ARGUMENT_ORDER: &str = include_str!("argument_order.json");

/// The resource names in registration order (`server.resource` calls in `server.py`).
const RESOURCE_ORDER: [&str; 2] = ["res_workspaces", "res_catalog"];

/// The resource-template names in registration order.
const TEMPLATE_ORDER: [&str; 5] = [
    "res_workspace",
    "res_project",
    "res_task",
    "res_history",
    "res_metric",
];

/// One tool: its wire descriptor and the model its arguments are checked against.
#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: String,
    /// The `tools/list` item: `name`, `description`, `inputSchema`, `outputSchema`.
    pub descriptor: Value,
    /// The arguments model, titled `<name>Arguments` as in the schema.
    pub arguments: Model,
}

/// Every descriptor the server lists.
#[derive(Clone, Debug)]
pub struct Catalog {
    pub tools: Vec<ToolSpec>,
    /// `resources/list` items.
    pub resources: Vec<Value>,
    /// `resources/templates/list` items.
    pub resource_templates: Vec<Value>,
}

fn broken(what: impl Into<String>) -> WgError {
    WgError::new(
        ErrorCode::WorkgraphError,
        format!("embedded mcp snapshot: {}", what.into()),
    )
}

fn text<'a>(item: &'a Value, key: &str, what: &str) -> Result<&'a str, WgError> {
    item.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| broken(format!("{what} has no `{key}`")))
}

fn field(item: &Value, key: &str, what: &str) -> Result<Value, WgError> {
    item.get(key)
        .cloned()
        .ok_or_else(|| broken(format!("{what} has no `{key}`")))
}

/// Items of `list` in the order `order` names them; an item missing from `order` goes last, by
/// name, and a name in `order` without an item is an error.
fn ordered(list: &[Value], order: &[&str], what: &str) -> Result<Vec<Value>, WgError> {
    let mut rest: Vec<&Value> = list.iter().collect();
    let mut out = Vec::new();
    for name in order {
        let at = rest
            .iter()
            .position(|item| item.get("name").and_then(Value::as_str) == Some(name))
            .ok_or_else(|| broken(format!("{what} `{name}` is not in the snapshot")))?;
        out.push(rest.remove(at).clone());
    }
    rest.sort_by_key(|item| item.get("name").and_then(Value::as_str).unwrap_or(""));
    out.extend(rest.into_iter().cloned());
    Ok(out)
}

fn array<'a>(document: &'a Value, key: &str) -> Result<&'a [Value], WgError> {
    document
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| broken(format!("no `{key}`")))
}

/// Puts the keys of `properties` in the order of `order`; keys it does not name keep their
/// relative order after the named ones.
fn reorder(properties: &mut Value, order: &[&str]) {
    let Some(map) = properties.as_object_mut() else {
        return;
    };
    let mut rest = std::mem::take(map);
    for name in order {
        if let Some(value) = rest.remove(*name) {
            map.insert((*name).to_string(), value);
        }
    }
    map.extend(rest);
}

fn as_refs(names: &[String]) -> Vec<&str> {
    names.iter().map(String::as_str).collect()
}

fn names(order: &Value, path: &[&str]) -> Vec<String> {
    path.iter()
        .try_fold(order, |at, key| at.get(*key))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A copy of the tool's input schema with `properties` in declaration order.
fn in_declaration_order(name: &str, input: &Value, order: &Value) -> Value {
    let mut schema = input.clone();
    if let Some(properties) = schema.get_mut("properties") {
        reorder(properties, &as_refs(&names(order, &["tools", name])));
    }
    if let Some(defs) = schema.get_mut("$defs").and_then(Value::as_object_mut) {
        for (def, body) in defs.iter_mut() {
            if let Some(properties) = body.get_mut("properties") {
                reorder(properties, &as_refs(&names(order, &["defs", def])));
            }
        }
    }
    schema
}

fn tool(item: &Value, order: &Value) -> Result<ToolSpec, WgError> {
    let name = text(item, "name", "tool")?;
    let input = field(item, "input_schema", name)?;
    let ordered_input = in_declaration_order(name, &input, order);
    let arguments = model_from_json_schema(&ordered_input, &ordered_input, LoadOptions::default())
        .map_err(|err| broken(format!("{name}: {err}")))?;
    let mut descriptor = Map::new();
    descriptor.insert("name".into(), json!(name));
    descriptor.insert(
        "description".into(),
        json!(text(item, "description", name)?),
    );
    descriptor.insert("inputSchema".into(), input);
    descriptor.insert("outputSchema".into(), field(item, "output_schema", name)?);
    Ok(ToolSpec {
        name: name.to_string(),
        descriptor: Value::Object(descriptor),
        arguments,
    })
}

/// A resource or template descriptor with the wire's key names.
fn resource(item: &Value, uri_key: &str, wire_uri_key: &str) -> Result<Value, WgError> {
    let name = text(item, "name", "resource")?;
    let mut out = Map::new();
    out.insert(wire_uri_key.into(), json!(text(item, uri_key, name)?));
    out.insert("name".into(), json!(name));
    out.insert(
        "description".into(),
        json!(text(item, "description", name)?),
    );
    out.insert("mimeType".into(), json!(text(item, "mime_type", name)?));
    Ok(Value::Object(out))
}

impl Catalog {
    /// The catalog of the embedded snapshot.
    pub fn embedded() -> Result<Self, WgError> {
        let document = wg_wire::contract::mcp()?;
        let order: Vec<&str> = TOOL_ORDER.lines().filter(|l| !l.is_empty()).collect();
        let argument_order: Value = serde_json::from_str(ARGUMENT_ORDER)
            .map_err(|err| broken(format!("argument_order.json: {err}")))?;
        let tools = ordered(array(&document, "tools")?, &order, "tool")?
            .iter()
            .map(|item| tool(item, &argument_order))
            .collect::<Result<Vec<_>, _>>()?;
        let resources = ordered(array(&document, "resources")?, &RESOURCE_ORDER, "resource")?
            .iter()
            .map(|item| resource(item, "uri", "uri"))
            .collect::<Result<Vec<_>, _>>()?;
        let resource_templates = ordered(
            array(&document, "resource_templates")?,
            &TEMPLATE_ORDER,
            "resource template",
        )?
        .iter()
        .map(|item| resource(item, "uri_template", "uriTemplate"))
        .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            tools,
            resources,
            resource_templates,
        })
    }

    /// The tool called `name`.
    pub fn tool(&self, name: &str) -> Option<&ToolSpec> {
        self.tools.iter().find(|spec| spec.name == name)
    }

    /// The `tools/list` result.
    pub fn tools_list(&self) -> Value {
        json!({"tools": self.tools.iter().map(|t| t.descriptor.clone()).collect::<Vec<_>>()})
    }

    /// The `resources/list` result.
    pub fn resources_list(&self) -> Value {
        json!({"resources": self.resources})
    }

    /// The `resources/templates/list` result.
    pub fn resource_templates_list(&self) -> Value {
        json!({"resourceTemplates": self.resource_templates})
    }
}
