//! Parsing of control requests (pure, so it is unit tested without sockets).

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Snap { message: String },
    Checkpoint,
    Status,
}

impl Command {
    pub fn parse(line: &str) -> Result<Command, String> {
        let line = line.trim();
        let (verb, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        match verb {
            "snap" => {
                let message = rest.trim();
                Ok(Command::Snap {
                    message: if message.is_empty() {
                        "snapshot"
                    } else {
                        message
                    }
                    .to_string(),
                })
            }
            "checkpoint" => Ok(Command::Checkpoint),
            "status" => Ok(Command::Status),
            "" => Err("empty request".to_string()),
            other => Err(format!("unknown command {other:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(
            Command::parse("snap fix the thing\n").unwrap(),
            Command::Snap {
                message: "fix the thing".into()
            }
        );
        assert_eq!(
            Command::parse("snap").unwrap(),
            Command::Snap {
                message: "snapshot".into()
            }
        );
        assert_eq!(Command::parse(" status ").unwrap(), Command::Status);
        assert_eq!(Command::parse("checkpoint").unwrap(), Command::Checkpoint);
    }

    #[test]
    fn rejects_unknown_and_empty() {
        assert!(Command::parse("").is_err());
        assert!(Command::parse("rm -rf /").is_err());
    }
}
