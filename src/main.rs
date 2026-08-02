//! fakestream generates synthetic test video for people building AV players.

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
";

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let command = arguments.first().map(String::as_str).unwrap_or("serve");

    let options = match Options::parse(&arguments) {
        Ok(options) => options,
        Err(message) => fail(&message),
    };

    match command {
        "build" => build(&options.dir),
        "serve" => {
            build(&options.dir);
            let address = SocketAddr::from(([0, 0, 0, 0], options.port));
            let runtime = match tokio::runtime::Runtime::new() {
                Ok(runtime) => runtime,
                Err(error) => fail(&format!("could not start the runtime: {error}")),
            };
            if let Err(error) = runtime.block_on(serve::run(options.dir, address)) {
                fail(&error.to_string());
            }
        }
        "help" | "--help" | "-h" => print!("{USAGE}"),
        other => fail(&format!("unknown command {other}\n\n{USAGE}")),
    }
}

struct Options {
    dir: PathBuf,
    port: u16,
}

impl Options {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut dir = PathBuf::from("fixtures");
        let mut port = 8080u16;

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
                other => return Err(format!("unknown option {other}\n\n{USAGE}")),
            }
        }

        Ok(Self { dir, port })
    }
}

fn build(dir: &std::path::Path) {
    match fixtures::build_all(dir) {
        Ok(results) => {
            for (fixture, built) in results {
                let state = if built { "generated" } else { "cached" };
                println!("{state:>9}  {}", fixture.route);
            }
        }
        Err(error) => fail(&error.to_string()),
    }
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}
