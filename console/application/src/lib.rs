//! Typed Rust client for the Draft Console application protocol.
//!
//! This crate is the sole backend boundary used by `console/tui`. It reuses
//! `draft-ipc` for transport and does not know how Draft domain state is stored.

use draft_ipc::console_application::{
    CanonicalRevisions, ConsoleActionInvocation, ConsoleActionResult, ConsoleHandshakeRequest,
    ConsoleHandshakeResponse, ConsoleModelRequest, ConsoleOperationStatus, ConsoleProtocolVersion,
    ConsoleReadModel, ConsoleSubject, ConsoleWatchEvent, ConsoleWatchRequest, CONSOLE_CAPABILITIES,
};
use draft_ipc::contract_versions::IPC_HANDSHAKE_REQUEST;
use draft_ipc::{
    call, socket_path, ErrorObject, HandshakeRequest, HandshakeResponse, Request, IPC_CAPABILITIES,
    IPC_PROTOCOL,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CLIENT_NAME: &str = "draft-console-tui";
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub struct ClientError {
    pub code: String,
    pub message: String,
    pub details: Value,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ClientError {}

impl From<std::io::Error> for ClientError {
    fn from(error: std::io::Error) -> Self {
        Self {
            code: "CONSOLE_DISCONNECTED".into(),
            message: error.to_string(),
            details: Value::Null,
        }
    }
}

impl From<ErrorObject> for ClientError {
    fn from(error: ErrorObject) -> Self {
        Self {
            code: error.code,
            message: error.message,
            details: error.details,
        }
    }
}

#[derive(Debug)]
pub struct ConsoleClient {
    socket: PathBuf,
    request_serial: AtomicU64,
    client_instance_id: String,
    handshake: ConsoleHandshakeResponse,
}

impl ConsoleClient {
    pub fn connect() -> Result<Self, ClientError> {
        Self::connect_to(socket_path())
    }

    pub fn connect_to(socket: impl Into<PathBuf>) -> Result<Self, ClientError> {
        let socket = socket.into();
        let client_instance_id = format!("tui-{}-{}", std::process::id(), unix_time_ms());
        transport_handshake(&socket)?;
        let requested = ConsoleHandshakeRequest {
            protocol: ConsoleProtocolVersion::default(),
            client_name: CLIENT_NAME.into(),
            client_version: CLIENT_VERSION.into(),
            client_instance_id: client_instance_id.clone(),
            requested_capabilities: CONSOLE_CAPABILITIES
                .iter()
                .map(|capability| (*capability).to_string())
                .collect(),
        };
        let handshake = typed_call(
            &socket,
            Request::new(
                "console-application-handshake",
                "console.handshake",
                encode(requested)?,
            ),
        )?;
        Ok(Self {
            socket,
            request_serial: AtomicU64::new(1),
            client_instance_id,
            handshake,
        })
    }

    pub fn handshake(&self) -> &ConsoleHandshakeResponse {
        &self.handshake
    }

    pub fn supports(&self, capability: &str) -> bool {
        self.handshake
            .negotiated_capabilities
            .iter()
            .any(|negotiated| negotiated == capability)
    }

    pub fn snapshot(&self, subject: ConsoleSubject) -> Result<ConsoleReadModel, ClientError> {
        self.request(
            "console.snapshot",
            ConsoleModelRequest {
                application_session_id: self.handshake.application_session_id.clone(),
                subject,
            },
        )
    }

    pub fn watch(
        &self,
        after_cursor: Option<u64>,
        subjects: Vec<ConsoleSubject>,
    ) -> Result<ConsoleWatchEvent, ClientError> {
        self.request(
            "console.watch",
            ConsoleWatchRequest {
                application_session_id: self.handshake.application_session_id.clone(),
                after_cursor,
                subjects,
            },
        )
    }

    pub fn invoke(
        &self,
        capability: String,
        operation_id: String,
        expected_revisions: CanonicalRevisions,
        arguments: BTreeMap<String, Value>,
    ) -> Result<ConsoleActionResult, ClientError> {
        let params = ConsoleActionInvocation {
            application_session_id: self.handshake.application_session_id.clone(),
            invocation_capability: capability,
            operation_id: operation_id.clone(),
            expected_revisions,
            arguments,
        };
        let request = Request::new(
            self.next_request_id(),
            "console.action.invoke",
            encode(params)?,
        )
        .with_operation_id(operation_id);
        typed_call(&self.socket, request)
    }

    pub fn operation_status(
        &self,
        operation_id: &str,
    ) -> Result<ConsoleOperationStatus, ClientError> {
        self.request(
            "operation.status",
            serde_json::json!({ "operation_id": operation_id }),
        )
    }

    pub fn cancel_operation(
        &self,
        target_operation_id: &str,
        cancellation_operation_id: String,
    ) -> Result<ConsoleOperationStatus, ClientError> {
        typed_call(
            &self.socket,
            Request::new(
                self.next_request_id(),
                "operation.cancel",
                serde_json::json!({ "target_operation_id": target_operation_id }),
            )
            .with_operation_id(cancellation_operation_id),
        )
    }

    pub fn reconnect(&mut self) -> Result<(), ClientError> {
        transport_handshake(&self.socket)?;
        let requested = ConsoleHandshakeRequest {
            protocol: ConsoleProtocolVersion::default(),
            client_name: CLIENT_NAME.into(),
            client_version: CLIENT_VERSION.into(),
            client_instance_id: self.client_instance_id.clone(),
            requested_capabilities: CONSOLE_CAPABILITIES
                .iter()
                .map(|capability| (*capability).to_string())
                .collect(),
        };
        self.handshake = typed_call(
            &self.socket,
            Request::new(
                self.next_request_id(),
                "console.handshake",
                encode(requested)?,
            ),
        )?;
        Ok(())
    }

    fn request<T, P>(&self, method: &str, params: P) -> Result<T, ClientError>
    where
        T: DeserializeOwned,
        P: serde::Serialize,
    {
        typed_call(
            &self.socket,
            Request::new(self.next_request_id(), method, encode(params)?),
        )
    }

    fn next_request_id(&self) -> String {
        format!(
            "console-tui-{}",
            self.request_serial.fetch_add(1, Ordering::Relaxed)
        )
    }
}

fn transport_handshake(socket: &Path) -> Result<HandshakeResponse, ClientError> {
    typed_call(
        socket,
        Request::new(
            "console-transport-handshake",
            "service.handshake",
            encode(HandshakeRequest {
                protocol: IPC_PROTOCOL.into(),
                schema_version: IPC_HANDSHAKE_REQUEST,
                requested_capabilities: IPC_CAPABILITIES
                    .iter()
                    .map(|capability| (*capability).to_string())
                    .collect(),
                client_name: CLIENT_NAME.into(),
                client_version: CLIENT_VERSION.into(),
            })?,
        ),
    )
}

fn typed_call<T: DeserializeOwned>(socket: &Path, request: Request) -> Result<T, ClientError> {
    let response = call(socket, &request)?;
    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| {
                ErrorObject::new(
                    "INVALID_RESPONSE",
                    "daemon returned a failed response without a typed error",
                )
            })
            .into());
    }
    serde_json::from_value(response.result.unwrap_or(Value::Null)).map_err(|error| ClientError {
        code: "INVALID_RESPONSE".into(),
        message: error.to_string(),
        details: Value::Null,
    })
}

fn encode<T: serde::Serialize>(value: T) -> Result<Value, ClientError> {
    serde_json::to_value(value).map_err(|error| ClientError {
        code: "CLIENT_ENCODING_ERROR".into(),
        message: error.to_string(),
        details: Value::Null,
    })
}

fn unix_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[derive(Debug, Clone)]
pub struct ReconnectBackoff {
    next: Duration,
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        Self {
            next: Duration::from_millis(250),
        }
    }
}

impl ReconnectBackoff {
    pub fn next_delay(&mut self) -> Duration {
        let current = self.next;
        self.next = (self.next * 2).min(Duration::from_secs(5));
        current
    }

    pub fn reset(&mut self) {
        self.next = Duration::from_millis(250);
    }
}

#[derive(Debug, Default, Clone)]
pub struct RequestGenerations {
    latest: BTreeMap<String, u64>,
}

impl RequestGenerations {
    pub fn begin(&mut self, target: impl Into<String>) -> u64 {
        let generation = self.latest.entry(target.into()).or_default();
        *generation = generation.saturating_add(1);
        *generation
    }

    pub fn is_current(&self, target: &str, generation: u64) -> bool {
        self.latest.get(target).copied() == Some(generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_backoff_is_bounded_and_resettable() {
        let mut backoff = ReconnectBackoff::default();
        assert_eq!(backoff.next_delay(), Duration::from_millis(250));
        for _ in 0..10 {
            backoff.next_delay();
        }
        assert_eq!(backoff.next_delay(), Duration::from_secs(5));
        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_millis(250));
    }

    #[test]
    fn late_generations_are_rejected_per_target() {
        let mut generations = RequestGenerations::default();
        let old = generations.begin("change");
        let current = generations.begin("change");
        assert!(!generations.is_current("change", old));
        assert!(generations.is_current("change", current));
    }
}
