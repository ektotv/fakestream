//! Serving live streams over HTTP.
//!
//! One endless GET per viewer, which is the classic shape an IPTV provider
//! serves and what a player is most likely to meet.

use crate::media::mux::ClipSpec;
use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_core::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Generation runs on a blocking thread and hands chunks to the response
/// through a channel, since encoding a frame is far too slow to do on an async
/// runtime thread.
pub async fn stream(spec: ClipSpec) -> Response {
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Vec<u8>, String>>(8);

    std::thread::spawn(move || {
        let mut live = match crate::media::live::LiveStream::new(spec) {
            Ok(live) => live,
            Err(error) => {
                let _ = sender.blocking_send(Err(error.to_string()));
                return;
            }
        };

        match live.header() {
            Ok(header) if !header.is_empty() => {
                if sender.blocking_send(Ok(header)).is_err() {
                    return;
                }
            }
            Ok(_) => {}
            Err(error) => {
                let _ = sender.blocking_send(Err(error.to_string()));
                return;
            }
        }

        loop {
            // Pace against the stream's own start, so a slow encode is absorbed
            // rather than pushing every later frame back.
            let wait = live.wait_before_next();
            if !wait.is_zero() {
                std::thread::sleep(wait);
            }

            match live.next_chunk() {
                Ok(chunk) => {
                    // An empty chunk is normal, since encoders buffer.
                    if chunk.is_empty() {
                        continue;
                    }
                    // A closed channel means the viewer went away, which is the
                    // ordinary way a live stream ends.
                    if sender.blocking_send(Ok(chunk)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = sender.blocking_send(Err(error.to_string()));
                    return;
                }
            }
        }
    });

    let body = Body::from_stream(ChunkStream { receiver });

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "video/mp2t"),
            // Nothing about an endless stream should ever be cached.
            (header::CACHE_CONTROL, "no-store, no-cache, must-revalidate"),
        ],
        body,
    )
        .into_response()
}

struct ChunkStream {
    receiver: tokio::sync::mpsc::Receiver<Result<Vec<u8>, String>>,
}

impl Stream for ChunkStream {
    type Item = Result<Vec<u8>, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.receiver.poll_recv(context) {
            Poll::Ready(Some(Ok(chunk))) => Poll::Ready(Some(Ok(chunk))),
            Poll::Ready(Some(Err(message))) => {
                Poll::Ready(Some(Err(std::io::Error::other(message))))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
