//! Request validation as FastAPI does it before a handler runs: path and query parameters
//! against their schemas in the OpenAPI snapshot, then the JSON body against the body schema the
//! route declares. Every error is collected (path, then query, then body) into one 422; a body
//! that is not valid JSON fails alone with `json_invalid`, and one that is not text with
//! FastAPI's 400. An error that holds a NaN or an infinity cannot be rendered (FastAPI's encoder
//! refuses it), so the request fails with a 500 instead.

use serde_json::{Map, Value as Json, json};
use wg_validate::{
    Extra, Field, LoadOptions, Model, Schema, ValidationError, holds_non_finite,
    model_from_json_schema, validate,
};

use crate::form::{query_get, query_getlist};
use crate::pyjson::{Decoded, loads};
use crate::reply::{Reply, body_parse_error, internal_error, validation_reply};

/// Where a parameter comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    Path,
    Query,
}

impl Location {
    fn name(self) -> &'static str {
        match self {
            Location::Path => "path",
            Location::Query => "query",
        }
    }
}

/// One declared parameter, with a one-field model that validates it.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub location: Location,
    pub required: bool,
    /// A list parameter takes every value given (`getlist`); others take the last one.
    pub list: bool,
    model: Model,
}

/// The JSON body a route takes: its schema and whether it may be absent.
#[derive(Debug, Clone, PartialEq)]
pub struct BodySpec {
    pub schema: Schema,
    pub required: bool,
}

/// What a request declares, for validation.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Declared {
    pub params: Vec<Param>,
    pub body: Option<BodySpec>,
}

/// The operation object for `method path` in the OpenAPI document.
pub fn operation<'a>(openapi: &'a Json, method: &str, path: &str) -> Option<&'a Json> {
    openapi
        .get("paths")?
        .get(path)?
        .get(method.to_ascii_lowercase())
}

fn is_list_schema(schema: &Json) -> bool {
    schema.get("type").and_then(Json::as_str) == Some("array")
        || schema
            .get("anyOf")
            .and_then(Json::as_array)
            .is_some_and(|options| options.iter().any(is_list_schema))
}

/// The declared path and query parameters of an operation, in the document's order (FastAPI
/// lists path parameters before query parameters).
pub fn params(openapi: &Json, operation: &Json) -> Result<Vec<Param>, String> {
    operation
        .get("parameters")
        .and_then(Json::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|param| {
            let name = param
                .get("name")
                .and_then(Json::as_str)
                .ok_or("parameter without a name")?;
            let location = match param.get("in").and_then(Json::as_str) {
                Some("path") => Location::Path,
                Some("query") => Location::Query,
                other => return Err(format!("parameter {name} in {other:?}")),
            };
            let required = param.get("required").and_then(Json::as_bool) == Some(true);
            let schema = param.get("schema").cloned().unwrap_or_else(|| json!({}));
            let object = json!({
                "type": "object",
                "title": name,
                "properties": {name: schema.clone()},
                "required": if required { vec![name] } else { Vec::new() },
            });
            let model = model_from_json_schema(openapi, &object, LoadOptions::default())
                .map_err(|err| format!("parameter {name}: {err}"))?;
            Ok(Param {
                name: name.to_owned(),
                location,
                required,
                list: is_list_schema(&schema),
                model,
            })
        })
        .collect()
}

fn located(errors: Vec<ValidationError>, location: &str) -> Vec<Json> {
    errors
        .into_iter()
        .map(|err| {
            let mut item = err.to_json();
            if let Some(loc) = item.get_mut("loc").and_then(Json::as_array_mut) {
                loc.insert(0, Json::String(location.to_owned()));
            }
            item
        })
        .collect()
}

fn missing(loc: Vec<Json>) -> Json {
    json!({"type": "missing", "loc": loc, "msg": "Field required", "input": null})
}

/// Validates the parameters: the values by name, or every error item.
pub fn check_params(
    declared: &[Param],
    path_params: &[(String, String)],
    query: &[(String, String)],
) -> (Map<String, Json>, Vec<Json>) {
    declared.iter().fold(
        (Map::new(), Vec::new()),
        |(mut values, mut errors), param| {
            let given = match param.location {
                Location::Path => path_params
                    .iter()
                    .find(|(key, _)| key == &param.name)
                    .map(|(_, value)| Json::String(value.clone())),
                Location::Query if param.list => Some(query_getlist(query, &param.name))
                    .filter(|list| !list.is_empty())
                    .map(|list| Json::Array(list.into_iter().map(Json::String).collect())),
                Location::Query => query_get(query, &param.name).map(Json::String),
            };
            let input = match &given {
                Some(value) => json!({param.name.clone(): value}),
                None if param.required => {
                    errors.push(missing(vec![
                        json!(param.location.name()),
                        json!(param.name),
                    ]));
                    return (values, errors);
                }
                None => json!({}),
            };
            match validate(&param.model, &input) {
                Ok(Json::Object(done)) => values.extend(done),
                Ok(_) => {}
                Err(found) => errors.extend(located(found, param.location.name())),
            }
            (values, errors)
        },
    )
}

/// The media type check of FastAPI: a body is read as JSON when there is no `Content-Type`, or
/// when it is `application/json` or `application/*+json`.
pub fn is_json_content(content_type: Option<&str>) -> bool {
    let Some(value) = content_type.filter(|v| !v.is_empty()) else {
        return true;
    };
    let media = value
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match media.split_once('/') {
        Some(("application", sub)) => sub == "json" || sub.ends_with("+json"),
        _ => false,
    }
}

/// The body as FastAPI reads it: `None` when empty, the JSON value for a JSON body, the text for
/// any other body. `Err` is the reply that ends the request (a `json_invalid` 422 or the 400).
pub fn read_body(body: &[u8], content_type: Option<&str>) -> Result<Option<Json>, Reply> {
    if body.is_empty() {
        return Ok(None);
    }
    if !is_json_content(content_type) {
        // FastAPI decodes the bytes as UTF-8 strictly; a body that is not UTF-8 is a 500.
        return match std::str::from_utf8(body) {
            Ok(text) => Ok(Some(Json::String(text.to_owned()))),
            Err(_) => Err(internal_error()),
        };
    }
    match loads(body) {
        Decoded::Value(value) => Ok(Some(value)),
        Decoded::NotText => Err(body_parse_error()),
        Decoded::Invalid { message, position } => Err(validation_reply(&[json!({
            "type": "json_invalid",
            "loc": ["body", position],
            "msg": "JSON decode error",
            "input": {},
            "ctx": {"error": message},
        })])),
    }
}

fn top_model(schema: &Schema) -> bool {
    match schema {
        Schema::Model(_) => true,
        Schema::Nullable(inner) => top_model(inner),
        _ => false,
    }
}

/// Validates the body value: `null` (absent, or JSON `null`) is missing when the body is required
/// and `null` otherwise. A model body given something other than an object fails as FastAPI's
/// `model_attributes_type`.
pub fn check_body(spec: &BodySpec, body: Option<&Json>) -> Result<Json, Vec<Json>> {
    let Some(value) = body.filter(|v| !v.is_null()) else {
        return if spec.required {
            Err(vec![missing(vec![json!("body")])])
        } else {
            Ok(Json::Null)
        };
    };
    if top_model(&spec.schema) && !value.is_object() {
        return Err(vec![json!({
            "type": "model_attributes_type",
            "loc": ["body"],
            "msg": "Input should be a valid dictionary or object to extract fields from",
            "input": value,
        })]);
    }
    let wrapper = Model::new(
        "Body",
        Extra::Ignore,
        vec![Field::required("body", spec.schema.clone())],
    );
    match validate(&wrapper, &json!({"body": value})) {
        Ok(Json::Object(mut done)) => Ok(done.remove("body").unwrap_or(Json::Null)),
        Ok(_) => Ok(Json::Null),
        Err(found) => Err(found.iter().map(ValidationError::to_json).collect()),
    }
}

/// A request that passed validation: the parameters by name and the body (`null` when absent).
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    pub params: Map<String, Json>,
    pub body: Json,
}

/// The whole of FastAPI's request validation: the reply that ends the request, or the values.
pub fn check_request(
    declared: &Declared,
    path_params: &[(String, String)],
    query: &[(String, String)],
    body: &[u8],
    content_type: Option<&str>,
) -> Result<Checked, Reply> {
    let read = match &declared.body {
        Some(_) => read_body(body, content_type)?,
        None => None,
    };
    let (params, mut errors) = check_params(&declared.params, path_params, query);
    let body = match &declared.body {
        Some(spec) => match check_body(spec, read.as_ref()) {
            Ok(value) => value,
            Err(found) => {
                errors.extend(found);
                Json::Null
            }
        },
        None => Json::Null,
    };
    if errors.is_empty() {
        Ok(Checked { params, body })
    } else if errors.iter().any(holds_non_finite) {
        Err(internal_error())
    } else {
        Err(validation_reply(&errors))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form::parse_qsl;

    fn openapi() -> Json {
        wg_wire::contract::openapi().unwrap()
    }

    fn body_text(reply: Reply) -> String {
        String::from_utf8(reply.body).unwrap()
    }

    #[test]
    fn query_errors_like_fastapi() {
        let doc = openapi();
        let op = operation(&doc, "GET", "/api/v1/ws/{ws}/events").unwrap();
        let declared = Declared {
            params: params(&doc, op).unwrap(),
            body: None,
        };
        let path = vec![("ws".to_owned(), "default".to_owned())];
        let checked = check_request(&declared, &path, &parse_qsl("limit=abc&limit=7"), b"", None);
        let values = checked.unwrap().params;
        assert_eq!(values["limit"], json!(7));
        assert_eq!(values["descending"], json!(false));
        assert_eq!(values["ws"], json!("default"));
        let reply =
            check_request(&declared, &path, &parse_qsl("limit=abc"), b"", None).unwrap_err();
        assert_eq!(reply.status, 422);
        assert_eq!(
            body_text(reply),
            r#"{"error":{"code":"validation_failed","message":"invalid request","details":{"errors":[{"type":"int_parsing","loc":["query","limit"],"msg":"Input should be a valid integer, unable to parse string as an integer","input":"abc"}]}}}"#
        );
        let both = check_request(
            &declared,
            &path,
            &parse_qsl("limit=9999999&descending=maybe"),
            b"",
            None,
        )
        .unwrap_err();
        assert_eq!(
            body_text(both),
            r#"{"error":{"code":"validation_failed","message":"invalid request","details":{"errors":[{"type":"less_than_equal","loc":["query","limit"],"msg":"Input should be less than or equal to 5000","input":"9999999","ctx":{"le":5000}},{"type":"bool_parsing","loc":["query","descending"],"msg":"Input should be a valid boolean, unable to interpret input","input":"maybe"}]}}}"#
        );
    }

    #[test]
    fn required_and_list_params() {
        let doc = openapi();
        let op = operation(&doc, "GET", "/api/v1/attempts/compare").unwrap();
        let declared = Declared {
            params: params(&doc, op).unwrap(),
            body: None,
        };
        let reply = check_request(&declared, &[], &[], b"", None).unwrap_err();
        assert_eq!(
            body_text(reply),
            r#"{"error":{"code":"validation_failed","message":"invalid request","details":{"errors":[{"type":"missing","loc":["query","a"],"msg":"Field required","input":null},{"type":"missing","loc":["query","b"],"msg":"Field required","input":null}]}}}"#
        );
        let tasks = operation(&doc, "GET", "/api/v1/ws/{ws}/tasks").unwrap();
        let declared = Declared {
            params: params(&doc, tasks).unwrap(),
            body: None,
        };
        let path = vec![("ws".to_owned(), "w".to_owned())];
        let ok = check_request(
            &declared,
            &path,
            &parse_qsl("status=ready&status=blocked"),
            b"",
            None,
        )
        .unwrap();
        assert_eq!(ok.params["status"], json!(["ready", "blocked"]));
        let bad =
            check_request(&declared, &path, &parse_qsl("status=bogus"), b"", None).unwrap_err();
        let text = body_text(bad);
        assert!(
            text.contains(r#""type":"enum","loc":["query","status",0]"#),
            "{text}"
        );
    }

    #[test]
    fn path_params_are_validated() {
        let doc = openapi();
        let op = operation(
            &doc,
            "POST",
            "/api/v1/verification/configs/{config_id}/{action}",
        )
        .unwrap();
        let declared = Declared {
            params: params(&doc, op).unwrap(),
            body: None,
        };
        let path = vec![
            ("config_id".to_owned(), "x".to_owned()),
            ("action".to_owned(), "frob".to_owned()),
        ];
        let reply = check_request(&declared, &path, &[], b"", None).unwrap_err();
        assert_eq!(
            body_text(reply),
            r#"{"error":{"code":"validation_failed","message":"invalid request","details":{"errors":[{"type":"literal_error","loc":["path","action"],"msg":"Input should be 'enable' or 'disable'","input":"frob","ctx":{"expected":"'enable' or 'disable'"}}]}}}"#
        );
    }

    #[test]
    fn bodies_like_fastapi() {
        let doc = openapi();
        let schema = json!({"$ref": "#/components/schemas/StackCreate"});
        let model = model_from_json_schema(&doc, &schema, LoadOptions::default()).unwrap();
        let declared = Declared {
            params: Vec::new(),
            body: Some(BodySpec {
                schema: Schema::Model(Box::new(model)),
                required: true,
            }),
        };
        let run = |body: &[u8], content_type: Option<&str>| match check_request(
            &declared,
            &[],
            &[],
            body,
            content_type,
        ) {
            Ok(checked) => Ok(checked.body),
            Err(reply) => Err((reply.status, body_text(reply))),
        };
        let json = Some("application/json");
        assert_eq!(
            run(br#"{"parent": "WG-1"}"#, json),
            Ok(json!({"parent": "WG-1"}))
        );
        assert_eq!(
            run(b"{", json).unwrap_err().1,
            r#"{"error":{"code":"validation_failed","message":"invalid request","details":{"errors":[{"type":"json_invalid","loc":["body",1],"msg":"JSON decode error","input":{},"ctx":{"error":"Expecting property name enclosed in double quotes"}}]}}}"#
        );
        assert_eq!(
            run(b"\xff", json).unwrap_err(),
            (
                400,
                r#"{"detail":"There was an error parsing the body"}"#.to_owned()
            )
        );
        let missing = r#"{"error":{"code":"validation_failed","message":"invalid request","details":{"errors":[{"type":"missing","loc":["body"],"msg":"Field required","input":null}]}}}"#;
        assert_eq!(run(b"", None).unwrap_err().1, missing);
        assert_eq!(run(b"null", json).unwrap_err().1, missing);
        assert_eq!(
            run(b"[]", json).unwrap_err().1,
            r#"{"error":{"code":"validation_failed","message":"invalid request","details":{"errors":[{"type":"model_attributes_type","loc":["body"],"msg":"Input should be a valid dictionary or object to extract fields from","input":[]}]}}}"#
        );
        assert_eq!(
            run(b"{}", Some("text/plain")).unwrap_err().1,
            r#"{"error":{"code":"validation_failed","message":"invalid request","details":{"errors":[{"type":"model_attributes_type","loc":["body"],"msg":"Input should be a valid dictionary or object to extract fields from","input":"{}"}]}}}"#
        );
        assert_eq!(
            run(br#"{"parent": 5}"#, json).unwrap_err().1,
            r#"{"error":{"code":"validation_failed","message":"invalid request","details":{"errors":[{"type":"string_type","loc":["body","parent"],"msg":"Input should be a valid string","input":5}]}}}"#
        );
        assert!(is_json_content(Some(
            "application/merge-patch+json; charset=utf-8"
        )));
        assert!(!is_json_content(Some("text/json")));
    }

    fn task_body(bytes: &[u8], content_type: Option<&str>) -> Result<Json, (u16, String)> {
        let doc = openapi();
        let schema = json!({"$ref": "#/components/schemas/TaskCreate"});
        let model = model_from_json_schema(&doc, &schema, LoadOptions::default()).unwrap();
        let declared = Declared {
            params: Vec::new(),
            body: Some(BodySpec {
                schema: Schema::Model(Box::new(model)),
                required: true,
            }),
        };
        match check_request(&declared, &[], &[], bytes, content_type) {
            Ok(checked) => Ok(checked.body),
            Err(reply) => Err((reply.status, body_text(reply))),
        }
    }

    #[test]
    fn non_finite_floats_like_python() {
        let json = Some("application/json");
        // Infinity passes `ge=0` and is stored as a float Python writes as null.
        let accepted = task_body(
            br#"{"title": "t", "estimated_effort": {"value": Infinity}}"#,
            json,
        )
        .unwrap();
        assert_eq!(accepted["estimated_effort"]["value"], Json::Null);
        assert_eq!(accepted["estimated_effort"]["unit"], json!("points"));
        // NaN and -Infinity fail `ge=0`; FastAPI cannot render an error that holds them: a 500.
        for body in [
            r#"{"title": "t", "estimated_effort": {"value": NaN}}"#,
            r#"{"title": "t", "estimated_effort": {"value": -Infinity}}"#,
            r#"{"title": "t", "parent": NaN}"#,
            r#"{"title": "t", "tags": [Infinity]}"#,
            r#"{"title": "t", "extra": NaN}"#,
        ] {
            assert_eq!(
                task_body(body.as_bytes(), json),
                Err((500, "Internal Server Error".to_owned())),
                "{body}"
            );
        }
        // A bare NaN is not a model: the same 500.
        assert_eq!(
            task_body(b"NaN", json),
            Err((500, "Internal Server Error".to_owned()))
        );
        // The infinity is accepted, so only the other error (the unit) is reported: a 422 whose
        // items hold no non-finite value.
        let (status, text) = task_body(
            br#"{"title": "t", "estimated_effort": {"value": 1e999, "unit": "bogus"}}"#,
            json,
        )
        .unwrap_err();
        assert_eq!(status, 422);
        assert!(
            text.contains(r#""loc":["body","estimated_effort","unit"]"#),
            "{text}"
        );
    }

    #[test]
    fn utf16_and_utf32_bodies_like_python() {
        let json = Some("application/json");
        let text = r#"{"title": "hé"}"#;
        let utf16le: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut with_bom = vec![0xff, 0xfe];
        with_bom.extend(&utf16le);
        let checked = task_body(&with_bom, json).unwrap();
        assert_eq!(checked["title"], json!("h\u{e9}"));
        let utf32be: Vec<u8> = text
            .chars()
            .flat_map(|c| (c as u32).to_be_bytes())
            .collect();
        assert_eq!(
            task_body(&utf32be, json).unwrap()["title"],
            json!("h\u{e9}")
        );
        // A bad JSON body in UTF-16 reports its position in the decoded text.
        assert_eq!(task_body(b"\xff\xfe{\x00", json).unwrap_err().0, 422);
        // An odd UTF-16 byte count is not text: the 400.
        assert_eq!(
            task_body(&utf16le[..utf16le.len() - 1], json),
            Err((
                400,
                r#"{"detail":"There was an error parsing the body"}"#.to_owned()
            ))
        );
        // A body without a JSON media type is decoded as strict UTF-8: UTF-16 is a 500.
        assert_eq!(
            task_body(&with_bom, Some("text/plain")),
            Err((500, "Internal Server Error".to_owned()))
        );
        assert_eq!(
            task_body(b"\xe9", Some("text/plain")),
            Err((500, "Internal Server Error".to_owned()))
        );
    }
}
