//! Hand an image to the platform's burning tool and stream its progress.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(100);
/// How long the tool gets to shut down before it is killed outright.
const TERMINATE_GRACE: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BurnProgress {
    pub percent: Option<f64>,
    pub message: String,
}

/// Parse one `hdiutil -puppetstrings` line.
pub fn parse_hdiutil_line(line: &str) -> Option<BurnProgress> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    // The man page says PERCENTAGE, hdiutil has been seen printing PERCENT: take either.
    if let Some(v) = line.strip_prefix("PERCENTAGE:").or_else(|| line.strip_prefix("PERCENT:")) {
        let p: f64 = v.trim().parse().ok()?;
        return Some(BurnProgress { percent: (p >= 0.0).then_some(p), message: String::new() });
    }
    let msg = line.strip_prefix("MESSAGE:").unwrap_or(line).trim();
    Some(BurnProgress { percent: None, message: msg.to_string() })
}

/// Command line for the platforms that burn with an external tool. Windows burns in process
/// through IMAPI2 instead, so this is only reached from macOS and Linux.
pub fn burn_command(drive_id: &str, iso: &str, speed: u8, verify: bool, test_only: bool) -> Result<Vec<String>, String> {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (drive_id, iso, speed, verify, test_only);
        return Err("这个平台没有刻录后端".into());
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let _ = drive_id;
    #[cfg(target_os = "macos")]
    {
        let mut a = vec!["hdiutil".to_string(), "burn".into(), iso.into(), "-puppetstrings".into(), "-speed".into(), speed.to_string()];
        if !verify {
            a.push("-noverifyburn".into());
        }
        if test_only {
            a.push("-testburn".into());
        }
        return Ok(a);
    }
    #[cfg(target_os = "linux")]
    {
        let mut a = vec!["growisofs".to_string(), "-dvd-compat".into(), format!("-speed={speed}")];
        if test_only {
            a.push("-dry-run".into());
        }
        let _ = verify;
        a.push("-Z".into());
        a.push(format!("{drive_id}={iso}"));
        return Ok(a);
    }
    #[allow(unreachable_code)]
    Err("这个平台没有刻录后端".into())
}

/// The virtual drive's burn: it walks the same progress a real one reports and writes
/// nothing, so the whole burn page and what follows it can be tried without a disc.
pub fn burn_simulated(step: Duration, cancel: &AtomicBool, on_progress: &mut dyn FnMut(BurnProgress)) -> Result<(), String> {
    for percent in (0..=100).step_by(2) {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".into());
        }
        on_progress(BurnProgress { percent: Some(percent as f64), message: "模拟写入，不会写入任何光盘".into() });
        std::thread::sleep(step);
    }
    Ok(())
}

/// Asks the tool to stop, so it can release the drive itself.
fn terminate(child: &mut Child) {
    #[cfg(unix)]
    let _ = Command::new("kill").args(["-TERM", &child.id().to_string()]).status();
    #[cfg(not(unix))]
    let _ = child.kill();
}

/// Blocks until the tool exits. Its own error text goes back unchanged: a refused drive
/// or medium is reported better there than anything guessed here. Setting `cancel` stops
/// the burn and returns `已取消`; a disc stopped mid-write is lost.
pub fn burn(argv: &[String], cancel: &AtomicBool, on_progress: &mut dyn FnMut(BurnProgress)) -> Result<(), String> {
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动 {}：{e}", argv[0]))?;
    let stderr = child.stderr.take().unwrap();
    let err_thread = std::thread::spawn(move || {
        BufReader::new(stderr).lines().map_while(Result::ok).collect::<Vec<_>>().join("\n")
    });
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut last = String::new();
    let mut stopped_at: Option<Instant> = None;
    loop {
        match rx.recv_timeout(POLL) {
            Ok(line) => {
                if let Some(p) = parse_hdiutil_line(&line) {
                    if !p.message.is_empty() {
                        last = p.message.clone();
                    }
                    on_progress(p);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if cancel.load(Ordering::Relaxed) {
            match stopped_at {
                None => {
                    terminate(&mut child);
                    stopped_at = Some(Instant::now());
                }
                Some(t) if t.elapsed() > TERMINATE_GRACE => {
                    let _ = child.kill();
                    break;
                }
                Some(_) => {}
            }
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if stopped_at.is_some() {
        return Err("已取消".into());
    }
    let err = err_thread.join().unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        Err(if err.trim().is_empty() {
        format!("刻录失败：{last}（退出码 {}）", status.code().unwrap_or(-1))
    } else {
        err
    })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_puppet_strings() {
        assert_eq!(parse_hdiutil_line("PERCENT:42.5").unwrap().percent, Some(42.5));
        assert_eq!(parse_hdiutil_line("PERCENTAGE:7.5").unwrap().percent, Some(7.5));
        assert_eq!(parse_hdiutil_line("PERCENT:-1.000000").unwrap().percent, None);
        assert_eq!(parse_hdiutil_line("MESSAGE:Burning").unwrap().message, "Burning");
        assert!(parse_hdiutil_line("   ").is_none());
    }

    #[test]
    fn reports_tool_failure_text() {
        let argv = ["sh", "-c", "echo boom 1>&2; exit 3"].map(String::from);
        assert_eq!(burn(&argv, &AtomicBool::new(false), &mut |_| {}).unwrap_err(), "boom");
    }

    #[test]
    fn cancel_stops_the_tool_promptly() {
        let argv = ["sh", "-c", "echo PERCENT:10; exec sleep 30"].map(String::from);
        let cancel = AtomicBool::new(false);
        let t = Instant::now();
        let mut seen = None;
        let r = burn(&argv, &cancel, &mut |p| {
            seen = p.percent;
            cancel.store(true, Ordering::Relaxed);
        });
        assert_eq!(r.unwrap_err(), "已取消");
        assert_eq!(seen, Some(10.0));
        assert!(t.elapsed() < Duration::from_secs(5));
    }
}

#[cfg(test)]
mod sim_tests {
    use super::*;

    #[test]
    fn simulated_burn_reports_to_completion_and_obeys_cancel() {
        let cancel = AtomicBool::new(false);
        let mut last = None;
        burn_simulated(Duration::ZERO, &cancel, &mut |p| last = p.percent).unwrap();
        assert_eq!(last, Some(100.0));
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(burn_simulated(Duration::ZERO, &cancel, &mut |_| {}), Err("已取消".into()));
    }
}