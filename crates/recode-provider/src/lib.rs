//! Provider wire clients and registry.
//!
//! The runner only sees canonical [`recode_schema::llm::LlmEvent`]s. Each
//! adapter owns request/response translation for one wire protocol.

pub mod anthropic;
pub mod catalog;
pub mod error;
pub mod openai;
pub mod registry;
pub mod sse;

use async_trait::async_trait;
use futures_util::stream::BoxStream;
use recode_schema::llm::{LlmEvent, LlmRequest};
use tokio_util::sync::CancellationToken;

pub use error::Result;

/// One configured provider endpoint. Implementations must be cancellation
/// safe: dropping the returned stream aborts the background read task.
#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;
    fn default_model(&self) -> Option<&str> {
        None
    }
    async fn stream(
        &self,
        request: LlmRequest,
        cancel: CancellationToken,
    ) -> Result<BoxStream<'static, std::result::Result<LlmEvent, recode_schema::llm::LlmError>>>;
}
