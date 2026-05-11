//! Handler for `trusty-analyzer service` (macOS launchd integration).
//!
//! Mirrors the trusty-search service layout: a thin platform-gated dispatcher
//! that maps each `ServiceAction` to a single `launchctl` operation. On
//! non-macOS targets the entry point prints a clear message and exits 1.

use anyhow::Result;
use colored::Colorize;

/// Reverse-DNS label for the LaunchAgent. Used as the plist filename and the
/// `Label` key — both must match for `launchctl` lookups to work.
#[cfg(target_os = "macos")]
const LAUNCHD_LABEL: &str = "com.trusty.trusty-analyzer";

/// Subcommand actions for `trusty-analyzer service`.
///
/// Why: launchd is the canonical way to keep a long-lived foreground service
/// alive on macOS — wrapping plist mechanics in `service` subcommands keeps
/// users from having to hand-edit XML.
/// What: each variant maps to one `launchctl` operation (or `tail -F` for Logs).
/// Test: `cargo run -- service --help` lists the four actions; on Linux,
/// any action prints "not supported" and exits 1.
#[derive(Debug, Clone)]
pub enum ServiceAction {
    /// Install the LaunchAgent plist and load it.
    Install,
    /// Unload the LaunchAgent and remove the plist.
    Uninstall,
    /// Show launchd status for the agent.
    Status,
    /// Tail the launchd stdout / stderr logs.
    Logs,
}

/// Dispatch a `trusty-analyzer service <action>` invocation.
///
/// Why: launchd is macOS-specific; on other platforms we exit cleanly with a
/// clear message rather than emitting confusing plist errors.
/// What: macOS routes to `service_install` / `service_uninstall` /
/// `service_status` / `service_logs`. Non-macOS prints "not supported" and
/// exits 1.
/// Test: on Linux, every action exits 1 with the platform message;
/// on macOS, `service status` runs `launchctl print` without crashing.
pub fn run_service_action(action: ServiceAction) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        match action {
            ServiceAction::Install => service_install(),
            ServiceAction::Uninstall => service_uninstall(),
            ServiceAction::Status => service_status(),
            ServiceAction::Logs => service_logs(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = action;
        eprintln!(
            "{} `trusty-analyzer service` is not supported on this platform — \
             use your distro's service manager (systemd, OpenRC, etc.) directly.",
            "✗".red()
        );
        std::process::exit(1);
    }
}

#[cfg(target_os = "macos")]
fn launchd_plist_path() -> Result<std::path::PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not resolve $HOME"))?;
    Ok(home
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist")))
}

#[cfg(target_os = "macos")]
fn launchd_log_dir() -> Result<std::path::PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not resolve $HOME"))?;
    let dir = home.join("Library").join("Logs").join("trusty-analyzer");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Render the LaunchAgent plist body. Foreground mode (launchd owns lifecycle).
#[cfg(target_os = "macos")]
fn launchd_plist_body(exe: &std::path::Path, log_dir: &std::path::Path) -> String {
    let exe = exe.display();
    let stdout = log_dir.join("stdout.log");
    let stderr = log_dir.join("stderr.log");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCHD_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>serve</string>
        <string>--foreground</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <!-- KeepAlive=SuccessfulExit:false means launchd only restarts the daemon
         on a non-zero exit. Matches the trusty-search service contract so a
         clean shutdown (SIGTERM → exit 0) does not crash-loop. -->
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ThrottleInterval</key>
    <integer>30</integer>
    <key>StandardOutPath</key>
    <string>{}</string>
    <key>StandardErrorPath</key>
    <string>{}</string>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#,
        stdout.display(),
        stderr.display(),
    )
}

#[cfg(target_os = "macos")]
fn service_install() -> Result<()> {
    let exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("could not resolve current exe: {e}"))?;
    let plist_path = launchd_plist_path()?;
    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let log_dir = launchd_log_dir()?;
    let body = launchd_plist_body(&exe, &log_dir);
    std::fs::write(&plist_path, body)
        .map_err(|e| anyhow::anyhow!("write {}: {e}", plist_path.display()))?;
    println!(
        "{} Wrote LaunchAgent plist: {}",
        "✓".green(),
        plist_path.display()
    );

    // Bootstrap into the GUI domain of the current user. `bootout` first
    // (ignoring errors) so a re-install replaces a previously-loaded agent
    // cleanly.
    let uid = unsafe { libc::getuid() };
    let domain = format!("gui/{uid}");
    let _ = std::process::Command::new("launchctl")
        .args(["bootout", &domain])
        .arg(&plist_path)
        .status();
    let status = std::process::Command::new("launchctl")
        .args(["bootstrap", &domain])
        .arg(&plist_path)
        .status()
        .map_err(|e| anyhow::anyhow!("launchctl bootstrap failed: {e}"))?;
    if !status.success() {
        anyhow::bail!("launchctl bootstrap exited with {status}");
    }
    println!(
        "{} trusty-analyzer service installed and started ({} loaded into {}).",
        "✓".green(),
        LAUNCHD_LABEL,
        domain
    );
    println!(
        "  Logs:    {}\n  Status:  {}",
        log_dir.display().to_string().dimmed(),
        "trusty-analyzer service status".cyan(),
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn service_uninstall() -> Result<()> {
    let plist_path = launchd_plist_path()?;
    let uid = unsafe { libc::getuid() };
    let domain = format!("gui/{uid}");
    if plist_path.exists() {
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", &domain])
            .arg(&plist_path)
            .status();
        std::fs::remove_file(&plist_path)
            .map_err(|e| anyhow::anyhow!("remove {}: {e}", plist_path.display()))?;
        println!(
            "{} trusty-analyzer service uninstalled ({} removed).",
            "✓".green(),
            plist_path.display()
        );
    } else {
        println!(
            "{} {} not installed — nothing to do",
            "·".dimmed(),
            plist_path.display()
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn service_status() -> Result<()> {
    let uid = unsafe { libc::getuid() };
    let target = format!("gui/{uid}/{LAUNCHD_LABEL}");
    let output = std::process::Command::new("launchctl")
        .args(["print", &target])
        .output()
        .map_err(|e| anyhow::anyhow!("launchctl print failed: {e}"))?;
    if output.status.success() {
        println!("{}", String::from_utf8_lossy(&output.stdout));
    } else {
        // `launchctl print` exits non-zero when the service isn't loaded.
        eprintln!(
            "{} {} is not loaded ({})",
            "✗".red(),
            target,
            String::from_utf8_lossy(&output.stderr).trim()
        );
        eprintln!(
            "  Install with: {}",
            "trusty-analyzer service install".cyan()
        );
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn service_logs() -> Result<()> {
    use std::os::unix::process::CommandExt;
    let log_dir = launchd_log_dir()?;
    let stdout = log_dir.join("stdout.log");
    let stderr = log_dir.join("stderr.log");
    if !stdout.exists() && !stderr.exists() {
        eprintln!(
            "{} No logs at {} yet — start the service first.",
            "·".dimmed(),
            log_dir.display()
        );
        return Ok(());
    }
    // Replace the current process with `tail -F` so the user gets a familiar
    // follow-mode experience (Ctrl-C cleanly stops tail without leaving us in
    // an ambiguous state) and we don't have to re-implement log rotation.
    let err = std::process::Command::new("tail")
        .arg("-F")
        .arg(&stdout)
        .arg(&stderr)
        .exec();
    Err(anyhow::anyhow!("exec tail failed: {err}"))
}
