use agent_ime_ipc::{PROTOCOL_VERSION, Server, request};
use std::path::PathBuf;
use std::time::Duration;

const MAX_SERVE_COUNT: usize = 32;

fn parse_serve_count(arguments: &[String]) -> Result<usize, String> {
    if arguments.len() != 4 {
        return Err("serve-count requires exactly one count".into());
    }
    let value = &arguments[3];
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("serve-count must be an ASCII decimal integer".into());
    }
    let count = value
        .parse::<usize>()
        .map_err(|_| "serve-count is outside the supported integer range".to_string())?;
    if !(1..=MAX_SERVE_COUNT).contains(&count) {
        return Err(format!(
            "serve-count must be between 1 and {MAX_SERVE_COUNT}"
        ));
    }
    Ok(count)
}

fn main() {
    let arguments = std::env::args().collect::<Vec<_>>();
    if arguments.len() < 3 {
        eprintln!(
            "usage: agent-ime-ipc-spike <serve-once|serve-count|request> <socket> [count|message]"
        );
        std::process::exit(2);
    }
    let path = PathBuf::from(&arguments[2]);
    match arguments[1].as_str() {
        "serve-once" => {
            let mut server = Server::bind(&path).unwrap_or_else(|error| {
                eprintln!("{error}");
                std::process::exit(3)
            });
            server.serve_once().unwrap_or_else(|error| {
                eprintln!("{error}");
                std::process::exit(4)
            });
        }
        "serve-count" => {
            let count = parse_serve_count(&arguments).unwrap_or_else(|error| {
                eprintln!("{error}");
                std::process::exit(2)
            });
            let mut server = Server::bind(&path).unwrap_or_else(|error| {
                eprintln!("{error}");
                std::process::exit(3)
            });
            for _ in 0..count {
                server.serve_once().unwrap_or_else(|error| {
                    eprintln!("{error}");
                    std::process::exit(4)
                });
            }
        }
        "request" => {
            let message = arguments
                .get(3)
                .cloned()
                .unwrap_or_else(|| format!("{PROTOCOL_VERSION} health cli"));
            match request(&path, &message, Duration::from_millis(250)) {
                Ok(response) => print!("{response}"),
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(5);
                }
            }
        }
        _ => std::process::exit(2),
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_SERVE_COUNT, parse_serve_count};

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn serve_count_accepts_positive_values_within_the_limit() {
        assert_eq!(
            parse_serve_count(&arguments(&[
                "agent-ime-ipc-spike",
                "serve-count",
                "/tmp/control.sock",
                "1",
            ])),
            Ok(1)
        );
        assert_eq!(
            parse_serve_count(&arguments(&[
                "agent-ime-ipc-spike",
                "serve-count",
                "/tmp/control.sock",
                &MAX_SERVE_COUNT.to_string(),
            ])),
            Ok(MAX_SERVE_COUNT)
        );
    }

    #[test]
    fn serve_count_rejects_missing_extra_and_out_of_range_values() {
        for values in [
            vec!["agent-ime-ipc-spike", "serve-count", "/tmp/control.sock"],
            vec![
                "agent-ime-ipc-spike",
                "serve-count",
                "/tmp/control.sock",
                "1",
                "extra",
            ],
            vec![
                "agent-ime-ipc-spike",
                "serve-count",
                "/tmp/control.sock",
                "0",
            ],
            vec![
                "agent-ime-ipc-spike",
                "serve-count",
                "/tmp/control.sock",
                "33",
            ],
        ] {
            assert!(parse_serve_count(&arguments(&values)).is_err());
        }
    }

    #[test]
    fn serve_count_rejects_non_decimal_values() {
        for value in ["", "-1", "+1", "1.0", " 1", "1 ", "one"] {
            assert!(
                parse_serve_count(&arguments(&[
                    "agent-ime-ipc-spike",
                    "serve-count",
                    "/tmp/control.sock",
                    value,
                ]))
                .is_err()
            );
        }
    }
}
