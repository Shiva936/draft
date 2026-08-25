//! Local JSON-RPC-style IPC message shapes.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const IPC_PROTOCOL: &str = "draft-ipc";

pub const IPC_CAPABILITIES: &[&str] = &[
    "handshake",
    "correlation_ids",
    "operation_idempotency",
    "structured_errors",
    "progress_events",
    "cancellation",
    "workspace_revisions",
    "fenced_leases",
    "console_http",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub protocol: String,
    pub schema_version: u32,
    pub id: String,
    pub correlation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    pub method: String,
    pub params: Value,
}

impl draft_core::contracts::VersionedContract for Request {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::IpcRequest;
}

impl Request {
    pub fn new(id: impl Into<String>, method: impl Into<String>, params: Value) -> Self {
        let id = id.into();
        Request {
            protocol: IPC_PROTOCOL.into(),
            schema_version: draft_core::contracts::current_version(
                draft_core::contracts::ContractId::IpcRequest,
            ),
            correlation_id: id.clone(),
            id,
            operation_id: None,
            method: method.into(),
            params,
        }
    }

    pub fn with_operation_id(mut self, operation_id: impl Into<String>) -> Self {
        self.operation_id = Some(operation_id.into());
        self
    }

    pub fn validate(&self) -> Result<(), ErrorObject> {
        if self.protocol != IPC_PROTOCOL {
            return Err(ErrorObject::new(
                "UNSUPPORTED_PROTOCOL",
                format!(
                    "unsupported IPC protocol '{}'; expected '{IPC_PROTOCOL}'",
                    self.protocol
                ),
            ));
        }
        if !draft_core::contracts::supports_version(
            draft_core::contracts::ContractId::IpcRequest,
            self.schema_version,
        ) {
            return Err(ErrorObject::new(
                "UNSUPPORTED_SCHEMA",
                format!("unsupported IPC schema {}", self.schema_version),
            ));
        }
        if self.id.trim().is_empty()
            || self.correlation_id.trim().is_empty()
            || self.method.trim().is_empty()
            || self
                .operation_id
                .as_ref()
                .is_some_and(|operation_id| operation_id.trim().is_empty())
        {
            return Err(ErrorObject::new(
                "VALIDATION_ERROR",
                "IPC id, correlation_id, method, and any operation_id must be non-empty",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub protocol: String,
    pub schema_version: u32,
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObject>,
}

impl draft_core::contracts::VersionedContract for Response {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::IpcResponse;
}

impl Response {
    pub fn ok(id: impl Into<String>, result: Value) -> Self {
        Response {
            protocol: IPC_PROTOCOL.into(),
            schema_version: draft_core::contracts::current_version(
                draft_core::contracts::ContractId::IpcResponse,
            ),
            id: id.into(),
            ok: true,
            result: Some(result),
            error: None,
        }
    }
    pub fn err(id: impl Into<String>, error: ErrorObject) -> Self {
        Response {
            protocol: IPC_PROTOCOL.into(),
            schema_version: draft_core::contracts::current_version(
                draft_core::contracts::ContractId::IpcResponse,
            ),
            id: id.into(),
            ok: false,
            result: None,
            error: Some(error),
        }
    }

    pub fn validate(&self) -> Result<(), ErrorObject> {
        if self.protocol != IPC_PROTOCOL {
            return Err(ErrorObject::new(
                "UNSUPPORTED_PROTOCOL",
                format!("unsupported IPC response protocol '{}'", self.protocol),
            ));
        }
        if !draft_core::contracts::supports_version(
            draft_core::contracts::ContractId::IpcResponse,
            self.schema_version,
        ) {
            return Err(ErrorObject::new(
                "UNSUPPORTED_SCHEMA",
                format!("unsupported IPC response schema {}", self.schema_version),
            ));
        }
        let payload_valid = if self.ok {
            self.result.is_some() && self.error.is_none()
        } else {
            self.result.is_none() && self.error.is_some()
        };
        if !payload_valid {
            return Err(ErrorObject::new(
                "VALIDATION_ERROR",
                "IPC response must carry exactly one result or error matching ok",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HandshakeRequest {
    pub protocol: String,
    pub schema_version: u32,
    pub requested_capabilities: Vec<String>,
    pub client_name: String,
    pub client_version: String,
}

impl draft_core::contracts::VersionedContract for HandshakeRequest {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::IpcHandshakeRequest;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HandshakeResponse {
    pub protocol: String,
    pub schema_version: u32,
    pub capabilities: Vec<String>,
    pub daemon_name: String,
    pub daemon_version: String,
}

impl draft_core::contracts::VersionedContract for HandshakeResponse {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::IpcHandshakeResponse;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressEvent {
    pub protocol: String,
    pub schema_version: u32,
    pub correlation_id: String,
    pub operation_id: Option<String>,
    pub sequence: u64,
    pub phase: String,
    pub completed: u64,
    pub total: Option<u64>,
    pub message: String,
}

impl draft_core::contracts::VersionedContract for ProgressEvent {
    const CONTRACT: draft_core::contracts::ContractId =
        draft_core::contracts::ContractId::IpcProgressEvent;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorObject {
    pub code: String,
    pub message: String,
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
