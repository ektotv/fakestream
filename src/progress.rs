//! Showing what generation is doing.
//!
//! Building the catalogue takes a couple of minutes and used to print nothing
//! until each fixture finished, which is indistinguishable from being stuck.
//! That silence was mistaken for the cache not working, and an interrupted wait
//! genuinely did leave work to redo, so the two reinforced each other.

use crate::fixtures::Report;
use std::io::Write;

const BAR_WIDTH: usize = 24;

/// Draws a single updating line while a fixture generates, then leaves one
/// finished line behind per fixture.
pub struct Bar {
    /// Whether anything has been drawn that still needs clearing.
    line_open: bool,
    /// Which fixture of how many, shown alongside the bar.
    position: Option<(usize, usize)>,
    quiet: bool,
}

impl Bar {
    /// `quiet` drops the moving bar and keeps only the finished lines, which is
    /// what a log file or a pipe wants.
    pub fn new(quiet: bool) -> Self {
        Self {
            line_open: false,
            position: None,
            quiet,
        }
    }

    pub fn handle(&mut self, report: Report<'_>) {
        match report {
            Report::SweptPartials(count) => {
                let files = if count == 1 { "file" } else { "files" };
                println!("cleared {count} unfinished {files} from a previous run");
            }

            Report::Started {
                fixture,
                index,
                total,
            } => {
                if !self.quiet {
                    self.draw(fixture.route, 0.0, index + 1, total);
                }
            }

            Report::Progress { fixture, fraction } => {
                if !self.quiet {
                    self.redraw(fixture.route, fraction);
                }
            }

            Report::Finished { fixture, built } => {
                self.clear();
                let state = if built { "generated" } else { "cached" };
                println!("{state:>9}  {}", fixture.route);
            }
        }
    }

    fn draw(&mut self, route: &str, fraction: f64, index: usize, total: usize) {
        self.position = Some((index, total));
        self.redraw(route, fraction);
    }

    fn redraw(&mut self, route: &str, fraction: f64) {
        let filled = ((fraction * BAR_WIDTH as f64).round() as usize).min(BAR_WIDTH);
        let bar: String = "=".repeat(filled) + &" ".repeat(BAR_WIDTH - filled);
        let percent = (fraction * 100.0).round() as u32;

        let counter = match self.position {
            Some((index, total)) => format!("[{index}/{total}] "),
            None => String::new(),
        };

        print!("\r{counter}[{bar}] {percent:>3}%  {route}");
        let _ = std::io::stdout().flush();
        self.line_open = true;
    }

    /// Wipe the moving line before something else writes to the terminal.
    ///
    /// Anything printed while a bar is on screen lands in the middle of it,
    /// which is how the server's own startup line ended up spliced into one.
    pub fn interrupt(&mut self) {
        self.clear();
    }

    /// Wipe the moving line so the finished line does not land on top of it.
    fn clear(&mut self) {
        if self.line_open {
            print!("\r{}\r", " ".repeat(BAR_WIDTH + 48));
            let _ = std::io::stdout().flush();
            self.line_open = false;
        }
    }
}
