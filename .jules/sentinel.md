## 2025-01-24 - [Argument Injection & Orphaned Processes]
**Vulnerability:** Command-line flag injection via malicious item IDs and resource exhaustion from orphaned child processes on timeout.
**Learning:** CLI-wrapping tools that handle external IDs are vulnerable to flag injection if the ID starts with a hyphen. Additionally, `tokio::process::Command` does not kill child processes on drop by default, leading to orphaned processes if the future is timed out.
**Prevention:** Always use the `--` separator before positional arguments in CLI commands. Set `kill_on_drop(true)` on `tokio::process::Command` to ensure child processes are terminated when the execution future is dropped.
