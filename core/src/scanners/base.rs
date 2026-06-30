//! Shared [`BaseScanner`] implementation providing the
//! async subprocess helper used by every concrete scanner.

use crate::errors::{format_command, EngineError, ParseError};
use serde::de::DeserializeOwned;
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

/// Shared scanner utilities. Every concrete scanner embeds (or wraps)
/// a [`BaseScanner`] and delegates subprocess work to it.
#[derive(Debug, Clone)]
pub struct BaseScanner {
    pub source: &'static str,
    pub engine_kind: crate::models::Engine,
    pub binary: &'static str,
}

impl BaseScanner {
    pub const fn new(
        source: &'static str,
        engine_kind: crate::models::Engine,
        binary: &'static str,
    ) -> Self {
        Self {
            source,
            engine_kind,
            binary,
        }
    }

    /// Run *argv* and return ``(stdout, stderr)``. Raises
    /// [`EngineError`] if the subprocess exits non-zero or cannot be
    /// launched.
    pub async fn run(
        &self,
        argv: &[&str],
        timeout_secs: u64,
    ) -> Result<(String, String), EngineError> {
        if argv.is_empty() {
            return Err(EngineError::new(
                "refusing to run empty command",
                self.source,
                vec![],
                None,
                "",
            ));
        }
        let owned: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        let mut cmd = Command::new(argv[0]);
        // Security: Ensure the child process is killed if the future
        // is dropped (e.g. on timeout).
        cmd.kill_on_drop(true);
        for arg in &argv[1..] {
            cmd.arg(arg);
        }
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return Err(EngineError::new(
                    format!("failed to launch {}: {}", self.binary, e),
                    self.source,
                    owned,
                    None,
                    e.to_string(),
                ));
            }
        };
        let out = match timeout(Duration::from_secs(timeout_secs), async {
            let out = child.wait_with_output().await;
            out
        })
        .await
        {
            Ok(r) => r,
            Err(_) => {
                return Err(EngineError::new(
                    format!(
                        "{} timed out after {}s: {}",
                        self.binary,
                        timeout_secs,
                        format_command(&owned)
                    ),
                    self.source,
                    owned,
                    None,
                    "",
                ));
            }
        };
        let output = match out {
            Ok(o) => o,
            Err(e) => {
                return Err(EngineError::new(
                    format!("failed to read output from {}: {}", self.binary, e),
                    self.source,
                    owned,
                    None,
                    e.to_string(),
                ));
            }
        };
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

        if !output.status.success() {
            return Err(EngineError::new(
                format!(
                    "{} exited with {:?}: {}",
                    self.binary,
                    output.status.code(),
                    format_command(&owned)
                ),
                self.source,
                owned,
                output.status.code(),
                stderr.trim().to_string(),
            ));
        }
        Ok((stdout, stderr))
    }

    /// Convenience wrapper that runs *argv* and parses the stdout as JSON.
    pub async fn run_json<T: DeserializeOwned>(
        &self,
        argv: &[&str],
        timeout_secs: u64,
    ) -> Result<T, EngineError> {
        let (stdout, _stderr) = self.run(argv, timeout_secs).await?;
        serde_json::from_str(&stdout).map_err(|e| {
            EngineError::new(
                format!("failed to parse JSON from {}: {}", self.binary, e),
                self.source,
                argv.iter().map(|s| s.to_string()).collect(),
                None,
                e.to_string(),
            )
        })
    }

    /// Run *argv* and return the parsed output, wrapping any
    /// [`EngineError`] in a [`ParseError`].
    pub async fn run_with_parse<T, F>(
        &self,
        argv: &[&str],
        context: &str,
        parser: F,
        timeout_secs: u64,
    ) -> Result<T, EngineError>
    where
        F: FnOnce(&str) -> Result<T, ParseError>,
    {
        let (stdout, stderr) = self.run(argv, timeout_secs).await?;
        parser(&stdout).map_err(|e| {
            // Re-raise the parse error as an EngineError so the
            // caller has a single error type to deal with.
            EngineError::new(
                format!("{} parse error in {}: {}", self.source, context, e),
                self.source,
                argv.iter().map(|s| s.to_string()).collect(),
                None,
                format!("{}; stderr={}", e, stderr.trim()),
            )
        })
    }
}
