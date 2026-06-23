//! Real Slack Web API implementation of the `Poster` (inbound) and
//! `SlackClient` (outbound push) seams, backed by slack-morphism.
//!
//! Its methods are synchronous (matching the seams) and bridge to
//! slack-morphism's async API via a tokio runtime handle. `Handle::block_on`
//! must only be called from a non-runtime thread — the bridge dispatches on
//! plain OS threads, and the one-shot CLIs call it from `main` (outside the
//! runtime). See each binary for how the handle is obtained.

use std::sync::Arc;

use slack_morphism::prelude::*;
use tokio::runtime::Handle;
use tracing::error;

use crate::app::Poster;
use crate::notify::SlackClient as PushClient;

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone)]
pub struct RealSlack {
    client: Arc<SlackHyperClient>,
    token: SlackApiToken,
    rt: Handle,
}

impl RealSlack {
    pub fn new(client: Arc<SlackHyperClient>, token: SlackApiToken, rt: Handle) -> RealSlack {
        RealSlack { client, token, rt }
    }

    fn do_post(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        text: &str,
    ) -> Result<SlackTs, BoxErr> {
        let mut req = SlackApiChatPostMessageRequest::new(
            SlackChannelId(channel.to_string()),
            SlackMessageContent::new().with_text(text.to_string()),
        );
        if let Some(ts) = thread_ts {
            req = req.with_thread_ts(SlackTs(ts.to_string()));
        }
        let (client, token) = (self.client.clone(), self.token.clone());
        let resp = self
            .rt
            .block_on(async move { client.open_session(&token).chat_post_message(&req).await })?;
        Ok(resp.ts)
    }

    fn do_upload(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        filename: &str,
        title: &str,
        content: &str,
    ) -> Result<(), BoxErr> {
        let (client, token) = (self.client.clone(), self.token.clone());
        let (channel, filename, title) =
            (channel.to_string(), filename.to_string(), title.to_string());
        let thread = thread_ts.map(|s| s.to_string());
        let bytes = content.as_bytes().to_vec();
        self.rt.block_on(async move {
            let session = client.open_session(&token);
            let url = session
                .get_upload_url_external(&SlackApiFilesGetUploadUrlExternalRequest::new(
                    filename,
                    bytes.len(),
                ))
                .await?;
            session
                .files_upload_via_url(&SlackApiFilesUploadViaUrlRequest::new(
                    url.upload_url.clone(),
                    bytes,
                    "text/markdown".to_string(),
                ))
                .await?;
            let mut complete =
                SlackApiFilesCompleteUploadExternalRequest::new(vec![SlackApiFilesComplete::new(
                    url.file_id.clone(),
                )
                .with_title(title)])
                .with_channel_id(SlackChannelId(channel));
            if let Some(t) = thread {
                complete = complete.with_thread_ts(SlackTs(t));
            }
            session.files_complete_upload_external(&complete).await?;
            Ok::<(), BoxErr>(())
        })
    }
}

impl Poster for RealSlack {
    fn post(&self, channel: &str, thread_ts: Option<&str>, text: &str) {
        if let Err(e) = self.do_post(channel, thread_ts, text) {
            error!("chat.postMessage failed: {e}");
        }
    }
    fn upload(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        filename: &str,
        title: &str,
        content: &str,
    ) {
        if let Err(e) = self.do_upload(channel, thread_ts, filename, title, content) {
            error!("file upload failed: {e}");
        }
    }
}

impl PushClient for RealSlack {
    fn chat_post_message(&self, channel: &str, thread_ts: Option<&str>, text: &str) -> String {
        match self.do_post(channel, thread_ts, text) {
            Ok(ts) => ts.0,
            Err(e) => {
                error!("chat.postMessage failed: {e}");
                String::new()
            }
        }
    }
    fn files_upload_v2(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        filename: &str,
        title: &str,
        content: &str,
    ) {
        if let Err(e) = self.do_upload(channel, thread_ts, filename, title, content) {
            error!("file upload failed: {e}");
        }
    }
}

/// Build a Slack Web API client + bot token from config. Shared by all bins.
pub fn build_client(bot_token: &str) -> Result<(Arc<SlackHyperClient>, SlackApiToken), BoxErr> {
    let client = Arc::new(SlackClient::new(SlackClientHyperConnector::new()?));
    let token = SlackApiToken::new(SlackApiTokenValue(bot_token.to_string()));
    Ok((client, token))
}
