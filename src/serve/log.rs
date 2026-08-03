//! One line per HTTP request, so what a player actually asked for, and when,
//! is visible without reaching for a proxy.

use crate::progress::Bar;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Prints request lines without drawing over the progress bar.
pub struct RequestLog {
    /// The terminal is a single surface, shared with fixture generation.
    bar: Arc<Mutex<Bar>>,
}

impl RequestLog {
    pub fn new(bar: Arc<Mutex<Bar>>) -> Arc<Self> {
        Arc::new(Self { bar })
    }

    pub fn record(
        &self,
        client: SocketAddr,
        method: &str,
        status: u16,
        path: &str,
        elapsed_ms: u128,
    ) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let line = format_line(now, client, method, status, path, elapsed_ms);

        if let Ok(mut bar) = self.bar.lock() {
            bar.interrupt();
        }
        println!("{line}");
    }
}

/// The epoch time is reduced to a UTC time of day. The date would repeat on
/// every line, while the interesting part of a diagnostic read is the spacing
/// between requests.
fn format_line(
    epoch_millis: u128,
    client: SocketAddr,
    method: &str,
    status: u16,
    path: &str,
    elapsed_ms: u128,
) -> String {
    let day = epoch_millis % 86_400_000;
    let millis = day % 1_000;
    let seconds = (day / 1_000) % 60;
    let minutes = (day / 60_000) % 60;
    let hours = day / 3_600_000;

    format!(
        "{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z  {client}  {method} {status} {path} {elapsed_ms}ms"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> SocketAddr {
        "127.0.0.1:54321".parse().expect("socket address")
    }

    #[test]
    fn a_line_carries_everything_a_diagnosis_needs() {
        // 12:34:56.789 into the day.
        let line = format_line(
            45_296_789,
            client(),
            "GET",
            200,
            "/live/hls/stream0.m3u8",
            3,
        );

        assert_eq!(
            line,
            "12:34:56.789Z  127.0.0.1:54321  GET 200 /live/hls/stream0.m3u8 3ms"
        );
    }

    #[test]
    fn the_clock_wraps_at_midnight() {
        let one_second_past_midnight = 86_400_000 * 20_567 + 1_000;

        let line = format_line(
            one_second_past_midnight,
            client(),
            "GET",
            404,
            "/nothing",
            0,
        );

        assert!(line.starts_with("00:00:01.000Z"), "got: {line}");
        assert!(line.ends_with("0ms"), "got: {line}");
    }
}
