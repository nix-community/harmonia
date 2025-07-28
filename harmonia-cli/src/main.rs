use std::env;
use std::process::{Command, ExitStatus, exit};

fn run_nix_with_args(args: &[String]) -> Result<ExitStatus, std::io::Error> {
    let mut cmd = Command::new("nix");
    cmd.arg("--experimental-features")
        .arg("nix-command flakes")
        .args(args);
    cmd.status()
}

fn run(args: Vec<String>) -> i32 {
    match run_nix_with_args(&args) {
        Ok(status) => status.code().unwrap_or(1),
        Err(e) => {
            eprintln!("Failed to execute nix: {e}");
            1
        }
    }
}

fn main() {
    exit(run(env::args().skip(1).collect()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_with_version() {
        let args = vec!["--version".to_string()];
        let exit_code = run(args);
        assert_eq!(exit_code, 0, "nix --version should return exit code 0");
    }
}
