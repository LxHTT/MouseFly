//! Clipboard text sync (Phase 5).
//!
//! Sender polls the OS clipboard at 500 ms cadence and forwards changes as
//! `Frame::Clipboard` on the link's reliable control stream. Receiver writes
//! incoming text to the local clipboard.
//!
//! Echo suppression: every host keeps a "last seen" copy of the clipboard
//! text. The poller only fires `Clipboard` if the current value differs from
//! that watermark, AND the receiver updates the same watermark before writing
//! the inbound text. Without this two paired hosts ping-pong the clipboard
//! forever.

use std::sync::Arc;
use std::time::Duration;

use arboard::Clipboard;
use mousefly_core::Frame;
use tokio::sync::{mpsc::Sender, Mutex};
use tracing::{debug, warn};

const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Shared watermark — the clipboard text we most recently observed (either
/// because we polled it locally or because we just wrote it from the peer).
pub type Watermark = Arc<Mutex<Option<String>>>;

pub fn make_watermark() -> Watermark {
    Arc::new(Mutex::new(None))
}

/// Spawn a poller that watches the local clipboard and pushes `Frame::Clipboard`
/// onto `outbound` when the text changes. Lives until `outbound` closes.
pub fn spawn_poller(outbound: Sender<Frame>, mark: Watermark) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            let current = match read_clipboard().await {
                Ok(Some(t)) => t,
                Ok(None) => continue,
                Err(e) => {
                    debug!("clipboard read failed: {e:#}");
                    continue;
                }
            };
            let mut guard = mark.lock().await;
            if guard.as_deref() == Some(current.as_str()) {
                continue;
            }
            *guard = Some(current.clone());
            drop(guard);
            if outbound
                .send(Frame::Clipboard { text: current })
                .await
                .is_err()
            {
                break;
            }
        }
    });
}

/// Apply an inbound `Frame::Clipboard`. Updates the watermark first to
/// suppress an immediate echo from the next poll tick.
pub async fn apply(text: String, mark: &Watermark) {
    {
        let mut guard = mark.lock().await;
        *guard = Some(text.clone());
    }
    if let Err(e) = write_clipboard(text).await {
        warn!("clipboard write failed: {e:#}");
    }
}

async fn read_clipboard() -> std::result::Result<Option<String>, arboard::Error> {
    tokio::task::spawn_blocking(|| {
        let mut cb = Clipboard::new()?;
        cb.get_text().map(Some).or_else(|e| match e {
            arboard::Error::ContentNotAvailable => Ok(None),
            other => Err(other),
        })
    })
    .await
    .unwrap_or_else(|join_err| {
        Err(arboard::Error::Unknown {
            description: join_err.to_string(),
        })
    })
}

async fn write_clipboard(text: String) -> std::result::Result<(), arboard::Error> {
    tokio::task::spawn_blocking(move || {
        let mut cb = Clipboard::new()?;
        cb.set_text(text)
    })
    .await
    .unwrap_or_else(|join_err| {
        Err(arboard::Error::Unknown {
            description: join_err.to_string(),
        })
    })
}
