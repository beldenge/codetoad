use crate::grok_client::SearchMode;
use crate::protocol::{ChatCompletionResponse, ChatCompletionStreamChunk, ChatMessage, ChatTool};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub type StreamChunkHandler<'a> = dyn FnMut(ChatCompletionStreamChunk) -> Result<()> + Send + 'a;

#[async_trait::async_trait]
pub trait ModelClient: Send + Sync {
    fn set_model(&mut self, model: String);
    fn current_model(&self) -> &str;

    async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[ChatTool],
        search_mode: SearchMode,
    ) -> Result<ChatCompletionResponse>;

    async fn stream_chat(
        &self,
        messages: &[ChatMessage],
        tools: &[ChatTool],
        search_mode: SearchMode,
        cancel_token: &CancellationToken,
        on_chunk: &mut StreamChunkHandler<'_>,
    ) -> Result<()>;

    async fn plain_completion(&self, prompt: &str) -> Result<String>;
}
