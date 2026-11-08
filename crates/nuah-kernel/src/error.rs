// SPDX-License-Identifier: Apache-2.0
//! The error model ([ADR 0007]): a closed set of categories, plugin-owned reason codes and message keys.
//!
//! [ADR 0007]: https://docs.addaui.com/nuah/adr/0007-closed-error-categories-plugin-reason-codes-message-keys/

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The closed set of error categories. Each has one fixed meaning for retrying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// The input is wrong; retrying the same input never helps.
    Invalid,
    /// The thing does not exist (or the Subject may not know it exists).
    NotFound,
    /// The Subject is known but may not do this.
    Denied,
    /// No valid Identity; sign in and retry.
    Unauthenticated,
    /// The state changed under the caller (stale revision, already exists); re-read, then retry.
    Conflict,
    /// The Face and Core disagree on the contract version.
    Incompatible,
    /// This deployment cannot do it (a feature the data Adapter does not provide); retrying never helps.
    Unsupported,
    /// A bug or a failure of infrastructure; the caller can do nothing but retry later.
    Internal,
}

impl Category {
    pub fn http_status(self) -> u16 {
        match self {
            Category::Invalid => 400,
            Category::Unauthenticated => 401,
            Category::Denied => 403,
            Category::NotFound => 404,
            Category::Conflict => 409,
            Category::Incompatible => 426,
            Category::Unsupported => 501,
            Category::Internal => 500,
        }
    }
    pub fn retryable(self) -> bool {
        matches!(self, Category::Internal)
    }
}

/// An error that crosses the Link. `key` is a message key a Face can translate; `message` is the
/// English fallback; `reason` is a plugin-owned code (`tasks.stale_revision`) when one applies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Error {
    pub category: Category,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub key: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
    pub message: String,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(category: Category, key: &str, message: impl Into<String>) -> Self {
        Error { category, reason: None, key: key.into(), params: Value::Null, message: message.into() }
    }
    pub fn with_reason(mut self, reason: &str) -> Self {
        self.reason = Some(reason.into());
        self
    }
    pub fn with_params(mut self, params: Value) -> Self {
        self.params = params;
        self
    }
    pub fn invalid(field: &str, message: &str) -> Self {
        Error::new(Category::Invalid, "kernel.invalid", format!("{field}: {message}"))
            .with_params(json!({ "field": field, "problem": message }))
    }
    pub fn not_found(what: &str) -> Self {
        Error::new(Category::NotFound, "kernel.not_found", format!("{what} not found")).with_params(json!({ "what": what }))
    }
    pub fn denied(permission: &str) -> Self {
        Error::new(Category::Denied, "kernel.denied", format!("not allowed: {permission}"))
            .with_params(json!({ "permission": permission }))
    }
    pub fn unauthenticated() -> Self {
        Error::new(Category::Unauthenticated, "kernel.unauthenticated", "sign in first")
    }
    pub fn conflict(reason: &str, message: &str) -> Self {
        Error::new(Category::Conflict, reason, message).with_reason(reason)
    }
    pub fn unsupported(feature: &str, adapter: &str) -> Self {
        Error::new(Category::Unsupported, "kernel.unsupported", format!("the data Adapter {adapter} does not provide {feature}"))
            .with_params(json!({ "feature": feature, "adapter": adapter }))
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Error::new(Category::Internal, "kernel.internal", message)
    }
    /// A plugin-owned failure: the category plus a reason code under the plugin's namespace.
    pub fn reason(category: Category, reason: &str, message: &str) -> Self {
        Error::new(category, reason, message).with_reason(reason)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.category, self.message)
    }
}
impl std::error::Error for Error {}
