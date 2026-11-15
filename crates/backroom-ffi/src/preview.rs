//! Link previews for Swift: one [`LinkPreviewer`] for the app, independent
//! of any login or session.

use std::sync::Arc;

use backroom_core::preview::{self, Previewer};
use tokio::runtime::Runtime;

use crate::BackroomError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PreviewKind {
    /// A web page: title, description, maybe an image.
    Page,
    /// The link is an image.
    Image,
}

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct LinkPreview {
    /// Where the link ended up after redirects.
    pub url: String,
    pub kind: PreviewKind,
    pub title: Option<String>,
    pub description: Option<String>,
    /// `og:site_name`; show the host when missing.
    pub site_name: Option<String>,
    /// The page's image, or the linked image; PNG, JPEG, GIF, WebP, ….
    pub image: Option<Vec<u8>>,
}

impl std::fmt::Debug for LinkPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkPreview")
            .field("url", &self.url)
            .field("kind", &self.kind)
            .field("title", &self.title)
            .field("image_bytes", &self.image.as_ref().map(Vec::len))
            .finish_non_exhaustive()
    }
}

impl From<preview::LinkPreview> for LinkPreview {
    fn from(p: preview::LinkPreview) -> Self {
        LinkPreview {
            url: p.url.into(),
            kind: match p.kind {
                preview::PreviewKind::Page => PreviewKind::Page,
                preview::PreviewKind::Image => PreviewKind::Image,
            },
            title: p.title,
            description: p.description,
            site_name: p.site_name,
            image: p.image.map(|i| i.data),
        }
    }
}

/// Fetches link previews on its own small runtime.
#[derive(uniffi::Object)]
pub struct LinkPreviewer {
    runtime: Runtime,
    previewer: Previewer,
}

#[uniffi::export]
impl LinkPreviewer {
    #[uniffi::constructor]
    pub fn new() -> Result<Arc<Self>, BackroomError> {
        let internal = |e: &dyn std::fmt::Display| BackroomError::Internal {
            message: e.to_string(),
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("backroom-preview")
            .enable_all()
            .build()
            .map_err(|e| internal(&e))?;
        // reqwest's client wants a runtime around when it's built.
        let previewer = runtime
            .block_on(async { Previewer::new() })
            .map_err(|e| internal(&e))?;
        Ok(Arc::new(LinkPreviewer { runtime, previewer }))
    }

    /// The preview for `url`, or `None` when there's nothing to show (not
    /// a page or image, an error, a private address, …).
    pub async fn preview(&self, url: String) -> Option<LinkPreview> {
        let previewer = self.previewer.clone();
        let task = self
            .runtime
            .spawn(async move { previewer.preview(&url).await });
        match task.await {
            Ok(Ok(p)) => Some(p.into()),
            Ok(Err(e)) => {
                tracing::debug!("no link preview: {e}");
                None
            }
            Err(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_links_have_no_preview() {
        let previewer = LinkPreviewer::new().unwrap();
        let result = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(previewer.preview("http://127.0.0.1:9/".into()));
        assert_eq!(result, None);
    }
}
