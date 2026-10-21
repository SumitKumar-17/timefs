//! Parsing of control requests (pure, so it is unit tested without sockets).

use timefs_core::Hash;

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Snap {
        message: String,
    },
    Checkpoint,
    Status,
    /// Create a branch at a commit. Lets the CLI create branches while the mount holds the
    /// pack store's single writer lock.
    Branch {
        name: String,
        at: Hash,
    },
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
            "branch" => parse_branch(rest),
            "" => Err("empty request".to_string()),
            other => Err(format!("unknown command {other:?}")),
        }
    }
}

fn parse_branch(rest: &str) -> Result<Command, String> {
    let mut parts = rest.split_whitespace();
    let (Some(name), Some(at), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err("usage: branch <name> <commit hash>".to_string());
    };
    let at = Hash::from_hex(at).map_err(|e| format!("bad commit hash: {e}"))?;
    Ok(Command::Branch {
        name: name.to_string(),
        at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_requests() {
        let h = Hash([7; 32]);
        assert_eq!(
            Command::parse(&format!("branch feature/x {h}")).unwrap(),
            Command::Branch {
                name: "feature/x".into(),
                at: h
            }
        );
        assert!(Command::parse("branch onlyname").is_err());
        assert!(Command::parse("branch a nothex").is_err());
        assert!(Command::parse(&format!("branch a {h} extra")).is_err());
    }

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
