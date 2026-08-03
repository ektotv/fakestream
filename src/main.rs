//! fakestream generates synthetic test video for people building AV players.

use fakestream::media::{Loudness, set_loudness};
use fakestream::progress::Bar;
use fakestream::{fixtures, serve};
use std::net::SocketAddr;
use std::path::PathBuf;

const USAGE: &str = "\
fakestream generates synthetic test video for people building AV players.

usage:
  fakestream build [--dir PATH]
  fakestream serve [--dir PATH] [--port PORT]

options:
  --dir PATH    where generated fixtures are cached (default: ./fixtures)
  --port PORT   port to listen on (default: 8080)
  --quiet       drop the progress bar, keeping one line per fixture
  --verbose     let ffmpeg log everything, for diagnosing a bad file
  -v, --version print the version, commit and build date
";

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let command = arguments.first().map(String::as_str).unwrap_or("serve");

    // Before option parsing, so `fakestream serve --version` is not an error.
    if arguments
        .iter()
        .any(|argument| argument == "-v" || argument == "--version")
    {
        println!("{}", version_line());
        return;
    }

    let options = match Options::parse(&arguments) {
        Ok(options) => options,
        Err(message) => fail(&message),
    };

    set_loudness(if options.verbose {
        Loudness::Everything
    } else {
        Loudness::Errors
    });

    match command {
        "build" => build(&options.dir, options.quiet),
        "serve" => serve(options),
        "help" | "--help" | "-h" => print!("{USAGE}"),
        other => fail(&format!("unknown command {other}\n\n{USAGE}")),
    }
}

struct Options {
    dir: PathBuf,
    port: u16,
    quiet: bool,
    verbose: bool,
}

impl Options {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut dir = PathBuf::from("fixtures");
        let mut port = 8080u16;
        let mut quiet = false;
        let mut verbose = false;

        let mut rest = arguments.iter().skip(1);
        while let Some(flag) = rest.next() {
            match flag.as_str() {
                "--dir" => {
                    let value = rest.next().ok_or("--dir needs a path")?;
                    dir = PathBuf::from(value);
                }
                "--port" => {
                    let value = rest.next().ok_or("--port needs a number")?;
                    port = value
                        .parse()
                        .map_err(|_| format!("{value} is not a port"))?;
                }
                "--quiet" => quiet = true,
                "--verbose" => verbose = true,
                other => return Err(format!("unknown option {other}\n\n{USAGE}")),
            }
        }

        Ok(Self {
            dir,
            port,
            quiet,
            verbose,
        })
    }
}

/// Start listening straight away and generate in the background.
///
/// Generation takes a couple of minutes, and blocking the server on it means
/// nothing can even tell you what is happening. The index reports what is ready
/// and what is still building.
fn serve(options: Options) {
    let address = SocketAddr::from(([0, 0, 0, 0], options.port));
    let fixtures = fixtures::catalogue();
    let progress = serve::pending(&fixtures);

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => fail(&format!("could not start the runtime: {error}")),
    };

    // Bind and announce before generation starts. The generation thread draws a
    // progress bar, so anything printed after it begins lands in the middle of
    // that line.
    let listener = match runtime.block_on(serve::bind(address)) {
        Ok(listener) => listener,
        Err(error) => fail(&error.to_string()),
    };
    println!("serving on http://{address}");

    println!("fixtures are generated when first requested, then cached");

    if let Err(error) = runtime.block_on(serve::run(
        listener,
        address,
        options.dir,
        progress,
        options.quiet,
    )) {
        fail(&error.to_string());
    }
}

fn build(dir: &std::path::Path, quiet: bool) {
    let mut bar = Bar::new(quiet);
    if let Err(error) = fixtures::build_all(dir, &mut |report| bar.handle(report)) {
        fail(&error.to_string());
    }
}

/// The semver, then which commit and day produced the binary.
///
/// A bug report starts with which build produced it, and downloaded binaries
/// outlive anyone's memory of where they came from.
fn version_line() -> String {
    format!(
        "fakestream {} ({}, built {})",
        env!("CARGO_PKG_VERSION"),
        env!("FAKESTREAM_GIT_SHA"),
        env!("FAKESTREAM_BUILD_DATE"),
    )
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_line_opens_with_the_semver() {
        let expected = format!("fakestream {} (", env!("CARGO_PKG_VERSION"));
        assert!(version_line().starts_with(&expected));
    }

    #[test]
    fn the_build_date_is_a_calendar_date() {
        let line = version_line();
        let date = line
            .rsplit("built ")
            .next()
            .expect("a built section")
            .trim_end_matches(')');

        // Year first so lines sort, two digit month and day so widths agree.
        assert_eq!(date.len(), 10, "got: {date}");
        assert_eq!(&date[4..5], "-");
        assert_eq!(&date[7..8], "-");
        assert!(date.chars().filter(|c| c.is_ascii_digit()).count() == 8);
    }
}
