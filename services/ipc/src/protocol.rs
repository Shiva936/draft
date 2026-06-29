//! Local JSON-RPC-style IPC message shapes (TDD §8.4–8.5, FR-SVC-003).

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Request {
    pub fn new(id: impl Into<String>, method: impl Into<String>, params: Value) -> Self {
        Request {
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObject>,
}

impl Response {
    pub fn ok(id: impl Into<String>, result: Value) -> Self {
        Response {
            id: id.into(),
            ok: true,
            result: Some(result),
            error: None,
        }
    }
    pub fn err(id: impl Into<String>, error: ErrorObject) -> Self {
        Response {
            id: id.into(),
            ok: false,
            result: None,
            error: Some(error),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorObject {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: Value,
}

impl ErrorObject {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        ErrorObject {
            code: code.into(),
            message: message.into(),
            details: Value::Null,
        }
    }
}
