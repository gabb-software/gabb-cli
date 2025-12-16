#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

use anyhow::Result;

use super::workspace::TestWorkspace;

/// Runner for CLI commands against a test workspace
pub struct CliRunner<'a> {
    workspace: &'a TestWorkspace,
    args: Vec<String>,
    json: bool,
}

impl<'a> CliRunner<'a> {
    pub fn new(workspace: &'a TestWorkspace) -> Self {
        Self {
            workspace,
            args: Vec::new(),
            json: false,
        }
    }

    /// Run the symbols command
    pub fn symbols(mut self) -> Self {
        self.args.push("symbols".into());
        self
    }

    /// Run the symbol command
    pub fn symbol(mut self, name: &str) -> Self {
        self.args
            .extend(["symbol".into(), "--name".into(), name.into()]);
        self
    }

    /// Run the definition command
    pub fn definition(mut self) -> Self {
        self.args.push("definition".into());
        self
    }

    /// Run the implementation command
    pub fn implementation(mut self) -> Self {
        self.args.push("implementation".into());
        self
    }

    /// Run the usages command
    pub fn usages(mut self) -> Self {
        self.args.push("usages".into());
        self
    }

    /// Run the duplicates command
    pub fn duplicates(mut self) -> Self {
        self.args.push("duplicates".into());
        self
    }

    /// Add --file argument with position (line:col)
    pub fn at_file(mut self, path: impl AsRef<Path>, line: usize, col: usize) -> Self {
        let full_path = self.workspace.canonical_path(path);
        self.args.extend([
            "--file".into(),
            format!("{}:{}:{}", full_path.display(), line, col),
        ]);
        self
    }

    /// Add --file argument without position
    pub fn for_file(mut self, path: impl AsRef<Path>) -> Self {
        let full_path = self.workspace.canonical_path(path);
        self.args
            .extend(["--file".into(), full_path.to_string_lossy().into()]);
        self
    }

    /// Add --kind filter
    pub fn kind(mut self, kind: &str) -> Self {
        self.args.extend(["--kind".into(), kind.into()]);
        self
    }

    /// Add --limit
    pub fn limit(mut self, n: usize) -> Self {
        self.args.extend(["--limit".into(), n.to_string()]);
        self
    }

    /// Add --name filter
    pub fn name(mut self, name: &str) -> Self {
        self.args.extend(["--name".into(), name.into()]);
        self
    }

    /// Request JSON output
    pub fn json(mut self) -> Self {
        self.json = true;
        self
    }

    /// Add custom argument
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Execute the command and return output
    pub fn run(self) -> Result<CliOutput> {
        let mut cmd = Command::new(TestWorkspace::cli_bin());

        if self.json {
            cmd.arg("--json");
        }

        cmd.args(&self.args)
            .arg("--db")
            .arg(self.workspace.db_path())
            .current_dir(self.workspace.root());

        let output = cmd.output()?;

        Ok(CliOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status,
            json_mode: self.json,
        })
    }
}

/// Output from a CLI command
pub struct CliOutput {
    pub stdout: String,
    pub stderr: String,
    pub status: std::process::ExitStatus,
    json_mode: bool,
}

impl CliOutput {
    /// Assert command succeeded
    pub fn assert_success(&self) -> &Self {
        assert!(
            self.status.success(),
            "Command failed with status {:?}\nstderr: {}\nstdout: {}",
            self.status,
            self.stderr,
            self.stdout
        );
        self
    }

    /// Assert command failed
    pub fn assert_failure(&self) -> &Self {
        assert!(
            !self.status.success(),
            "Expected command to fail, but it succeeded.\nstdout: {}",
            self.stdout
        );
        self
    }

    /// Assert stdout contains string
    pub fn assert_stdout_contains(&self, needle: &str) -> &Self {
        assert!(
            self.stdout.contains(needle),
            "Expected stdout to contain '{}', got:\n{}",
            needle,
            self.stdout
        );
        self
    }

    /// Assert stdout does not contain string
    pub fn assert_stdout_not_contains(&self, needle: &str) -> &Self {
        assert!(
            !self.stdout.contains(needle),
            "Expected stdout NOT to contain '{}', got:\n{}",
            needle,
            self.stdout
        );
        self
    }

    /// Assert stderr contains string
    pub fn assert_stderr_contains(&self, needle: &str) -> &Self {
        assert!(
            self.stderr.contains(needle),
            "Expected stderr to contain '{}', got:\n{}",
            needle,
            self.stderr
        );
        self
    }

    /// Parse JSON output
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        assert!(self.json_mode, "Command was not run with --json flag");
        Ok(serde_json::from_str(&self.stdout)?)
    }

    /// Parse JSON output as generic Value
    pub fn json_value(&self) -> Result<serde_json::Value> {
        self.json()
    }

    /// Get stdout as string
    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    /// Get stderr as string
    pub fn stderr(&self) -> &str {
        &self.stderr
    }
}
