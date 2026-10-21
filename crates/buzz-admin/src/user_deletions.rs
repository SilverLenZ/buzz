//! Thin `buzz-admin user-deletions` adapter.

pub use buzz_deletion::user::Command as UserDeletionsCommand;

/// Delegate request intake to the shared deletion crate.
pub async fn run(command: UserDeletionsCommand) -> anyhow::Result<i32> {
    buzz_deletion::user::run(command).await
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    #[test]
    fn user_intake_commands_are_registered_without_execution() {
        for args in [
            vec![
                "buzz-admin",
                "user-deletions",
                "submit",
                "--target",
                "demo:alice",
                "--requested-by",
                "operator-a",
            ],
            vec![
                "buzz-admin",
                "user-deletions",
                "approve",
                "00000000-0000-0000-0000-000000000001",
                "--approved-by",
                "operator-b",
                "--inventory-digest",
                "digest-from-inspect",
            ],
            vec!["buzz-admin", "user-deletions", "list"],
            vec![
                "buzz-admin",
                "user-deletions",
                "inspect",
                "00000000-0000-0000-0000-000000000001",
            ],
        ] {
            assert!(crate::Cli::try_parse_from(args).is_ok());
        }
        assert!(crate::Cli::try_parse_from([
            "buzz-admin",
            "user-deletions",
            "approve",
            "00000000-0000-0000-0000-000000000001",
            "--approved-by",
            "operator-b",
        ])
        .is_err());
        for command in ["inventory", "run", "drain", "worker"] {
            assert!(crate::Cli::try_parse_from(["buzz-admin", "user-deletions", command]).is_err());
        }
    }
}
