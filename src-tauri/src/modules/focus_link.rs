//! `terax://focus?tty=..&pid=..&session=..`: an external app asks Terax to
//! bring forward the pane running a given terminal. The link is untrusted
//! input, so it can only select an existing pane; it never runs or opens
//! anything. The first parameter that resolves wins, in tty, pid, session order.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::Url;

#[cfg(target_os = "macos")]
pub const FOCUS_PANE_EVENT: &str = "terax:focus-pane";

const MAX_URL_LEN: usize = 2048;
const MAX_TTY_NAME: usize = 32;
const MAX_SESSION_FILES: usize = 1024;
const MAX_SESSION_FILE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct FocusRequest {
    pub tty: Option<PathBuf>,
    pub pid: Option<u32>,
    pub session: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FocusLinkError {
    TooLong,
    UnknownAction,
    InvalidParam(&'static str),
    NoTarget,
}

pub fn is_terax_link(url: &Url) -> bool {
    url.scheme() == "terax"
}

pub fn parse(url: &Url) -> Result<FocusRequest, FocusLinkError> {
    if url.as_str().len() > MAX_URL_LEN {
        return Err(FocusLinkError::TooLong);
    }
    if !is_terax_link(url)
        || url.host_str() != Some("focus")
        || !matches!(url.path(), "" | "/")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(FocusLinkError::UnknownAction);
    }
    let mut request = FocusRequest::default();
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "tty" if request.tty.is_none() => {
                request.tty =
                    Some(normalize_tty(&value).ok_or(FocusLinkError::InvalidParam("tty"))?);
            }
            "pid" if request.pid.is_none() => {
                request.pid = Some(parse_pid(&value).ok_or(FocusLinkError::InvalidParam("pid"))?);
            }
            "session" if request.session.is_none() => {
                request.session = Some(
                    normalize_session_id(&value).ok_or(FocusLinkError::InvalidParam("session"))?,
                );
            }
            _ => {}
        }
    }
    if request == FocusRequest::default() {
        return Err(FocusLinkError::NoTarget);
    }
    Ok(request)
}

/// `/dev/ttys012`, `ttys012` and `/dev/pts/3` style names; anything that could
/// escape `/dev` is refused.
pub fn normalize_tty(raw: &str) -> Option<PathBuf> {
    let name = raw.strip_prefix("/dev/").unwrap_or(raw);
    if name.is_empty() || name.len() > MAX_TTY_NAME {
        return None;
    }
    let valid = if let Some(n) = name.strip_prefix("pts/") {
        !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())
    } else {
        name.starts_with("tty") && name.bytes().all(|b| b.is_ascii_alphanumeric())
    };
    valid.then(|| Path::new("/dev").join(name))
}

fn parse_pid(raw: &str) -> Option<u32> {
    if raw.is_empty() || raw.len() > 10 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    raw.parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0 && *pid <= i32::MAX as u32)
}

/// Canonical 8-4-4-4-12 hex UUID, lowercased.
pub fn normalize_session_id(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    if bytes.len() != 36 {
        return None;
    }
    let shaped = bytes.iter().enumerate().all(|(i, b)| match i {
        8 | 13 | 18 | 23 => *b == b'-',
        _ => b.is_ascii_hexdigit(),
    });
    shaped.then(|| raw.to_ascii_lowercase())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionFile {
    pid: u32,
    session_id: String,
}

/// Claude Code writes `<pid>.json` with `{ pid, sessionId, .. }` for each live
/// session. Only plain `<digits>.json` files whose name agrees with their `pid`
/// are trusted.
pub fn session_pid(dir: &Path, session_id: &str) -> Option<u32> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten().take(MAX_SESSION_FILES) {
        let name = entry.file_name();
        let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".json")) else {
            continue;
        };
        let Some(file_pid) = parse_pid(stem) else {
            continue;
        };
        let Ok(meta) = entry.path().symlink_metadata() else {
            continue;
        };
        if !meta.is_file() || meta.len() > MAX_SESSION_FILE_BYTES {
            continue;
        }
        let Ok(contents) = std::fs::read(entry.path()) else {
            continue;
        };
        let Ok(file) = serde_json::from_slice::<SessionFile>(&contents) else {
            continue;
        };
        if file.pid == file_pid && file.session_id.eq_ignore_ascii_case(session_id) {
            return Some(file_pid);
        }
    }
    None
}

/// A live terminal that is bound to a frontend pane.
#[derive(Clone, Debug)]
pub struct PaneTty {
    pub pty_id: u32,
    pub pane_id: u32,
    pub owner: Arc<str>,
    pub tty: PathBuf,
}

/// OS lookups injected so the resolution order is testable without real ttys.
pub struct Probe<'a> {
    pub sessions_dir: Option<&'a Path>,
    /// Controlling terminal device of a process.
    pub pid_tty_dev: &'a dyn Fn(u32) -> Option<u64>,
    /// Device number of a tty path.
    pub tty_dev: &'a dyn Fn(&Path) -> Option<u64>,
}

pub fn resolve<'a>(
    request: &FocusRequest,
    panes: &'a [PaneTty],
    probe: &Probe,
) -> Option<&'a PaneTty> {
    let newest =
        |hit: &dyn Fn(&PaneTty) -> bool| panes.iter().filter(|p| hit(p)).max_by_key(|p| p.pty_id);
    let by_pid = |pid: u32| {
        let dev = (probe.pid_tty_dev)(pid)?;
        newest(&|p| (probe.tty_dev)(&p.tty) == Some(dev))
    };
    request
        .tty
        .as_deref()
        .and_then(|tty| newest(&|p| p.tty == tty))
        .or_else(|| request.pid.and_then(by_pid))
        .or_else(|| {
            let session = request.session.as_deref()?;
            by_pid(session_pid(probe.sessions_dir?, session)?)
        })
}

#[cfg(target_os = "macos")]
fn pid_tty_dev(pid: u32) -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let written = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    let dev = unsafe { info.assume_init() }.e_tdev;
    // NODEV: the process has no controlling terminal.
    (dev != u32::MAX && dev != 0).then_some(u64::from(dev))
}

#[cfg(target_os = "macos")]
fn tty_dev(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.rdev())
}

#[cfg(target_os = "macos")]
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FocusPanePayload {
    pane_id: u32,
}

#[cfg(target_os = "macos")]
pub fn handle(app: &tauri::AppHandle, url: &Url) {
    use tauri::{Emitter, Manager};

    let request = match parse(url) {
        Ok(request) => request,
        Err(error) => {
            log::warn!("ignored terax:// link: {error:?}");
            return;
        }
    };
    let panes = app
        .try_state::<super::pty::PtyState>()
        .map(|state| state.pane_ttys())
        .unwrap_or_default();
    let sessions_dir = dirs::home_dir().map(|home| home.join(".claude").join("sessions"));
    let probe = Probe {
        sessions_dir: sessions_dir.as_deref(),
        pid_tty_dev: &pid_tty_dev,
        tty_dev: &tty_dev,
    };
    let target = resolve(&request, &panes, &probe);
    let label = target.map_or("main", |pane| &*pane.owner);
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    // On a cold start the frontend reveals the window after first paint, so an
    // unmatched link must not show it early.
    if target.is_none()
        && !window.is_visible().unwrap_or(false)
        && !window.is_minimized().unwrap_or(false)
    {
        return;
    }
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    if let Some(pane) = target {
        let _ = app.emit_to(
            label,
            FOCUS_PANE_EVENT,
            FocusPanePayload {
                pane_id: pane.pane_id,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).expect("test url parses")
    }

    const SESSION: &str = "3f2b6c1e-9a4d-4e8b-b7c2-1d0e5f6a7b8c";

    #[test]
    fn parses_every_param() {
        let req = parse(&url(&format!(
            "terax://focus?tty=/dev/ttys012&pid=4242&session={}",
            SESSION.to_uppercase()
        )))
        .unwrap();
        assert_eq!(req.tty.as_deref(), Some(Path::new("/dev/ttys012")));
        assert_eq!(req.pid, Some(4242));
        assert_eq!(req.session.as_deref(), Some(SESSION));
    }

    #[test]
    fn accepts_trailing_slash_and_percent_encoding() {
        let req = parse(&url("terax://focus/?tty=%2Fdev%2Fttys003")).unwrap();
        assert_eq!(req.tty.as_deref(), Some(Path::new("/dev/ttys003")));
    }

    #[test]
    fn ignores_unknown_and_repeated_params() {
        let req = parse(&url("terax://focus?cmd=rm%20-rf&pid=7&pid=8&x=1")).unwrap();
        assert_eq!(
            req,
            FocusRequest {
                pid: Some(7),
                ..Default::default()
            }
        );
    }

    #[test]
    fn rejects_other_actions_and_shapes() {
        for raw in [
            "terax://run?tty=ttys001",
            "terax://open?tty=ttys001",
            "terax://focus/extra?tty=ttys001",
            "terax://user@focus?tty=ttys001",
            "terax://focus:99?tty=ttys001",
            "terax:focus?tty=ttys001",
            "https://focus?tty=ttys001",
        ] {
            assert_eq!(
                parse(&url(raw)),
                Err(FocusLinkError::UnknownAction),
                "{raw}"
            );
        }
    }

    #[test]
    fn requires_a_target() {
        assert_eq!(parse(&url("terax://focus")), Err(FocusLinkError::NoTarget));
        assert_eq!(
            parse(&url("terax://focus?foo=bar")),
            Err(FocusLinkError::NoTarget)
        );
    }

    #[test]
    fn rejects_malformed_values() {
        for (raw, param) in [
            ("terax://focus?tty=../etc/passwd", "tty"),
            ("terax://focus?tty=/dev/../etc/ttys1", "tty"),
            ("terax://focus?tty=/dev/disk0", "tty"),
            ("terax://focus?tty=", "tty"),
            ("terax://focus?pid=0", "pid"),
            ("terax://focus?pid=-1", "pid"),
            ("terax://focus?pid=12a", "pid"),
            ("terax://focus?pid=99999999999", "pid"),
            ("terax://focus?session=not-a-uuid", "session"),
            (
                "terax://focus?session=3f2b6c1e9a4d4e8bb7c21d0e5f6a7b8c",
                "session",
            ),
            (
                "terax://focus?session=3f2b6c1e-9a4d-4e8b-b7c2-1d0e5f6a7b8z",
                "session",
            ),
            (
                "terax://focus?session=../../3f2b6c1e-9a4d-4e8b-b7c2-1d0",
                "session",
            ),
        ] {
            assert_eq!(
                parse(&url(raw)),
                Err(FocusLinkError::InvalidParam(param)),
                "{raw}"
            );
        }
    }

    #[test]
    fn rejects_oversized_links() {
        let raw = format!("terax://focus?pid=1&pad={}", "a".repeat(MAX_URL_LEN));
        assert_eq!(parse(&url(&raw)), Err(FocusLinkError::TooLong));
    }

    #[test]
    fn normalizes_tty_names() {
        assert_eq!(
            normalize_tty("ttys012"),
            Some(PathBuf::from("/dev/ttys012"))
        );
        assert_eq!(
            normalize_tty("/dev/ttys012"),
            Some(PathBuf::from("/dev/ttys012"))
        );
        assert_eq!(
            normalize_tty("/dev/pts/3"),
            Some(PathBuf::from("/dev/pts/3"))
        );
        assert_eq!(normalize_tty("pts/"), None);
        assert_eq!(normalize_tty("/dev/tty/../x"), None);
        assert_eq!(normalize_tty("/dev/"), None);
        assert_eq!(normalize_tty(&format!("tty{}", "1".repeat(40))), None);
    }

    fn write_session(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn finds_the_pid_of_a_claude_session() {
        let dir = tempfile::tempdir().unwrap();
        write_session(
            dir.path(),
            "111.json",
            r#"{"pid":111,"sessionId":"00000000-0000-0000-0000-000000000000"}"#,
        );
        write_session(
            dir.path(),
            "222.json",
            &format!(
                r#"{{"pid":222,"sessionId":"{}","cwd":"/tmp"}}"#,
                SESSION.to_uppercase()
            ),
        );
        assert_eq!(session_pid(dir.path(), SESSION), Some(222));
    }

    #[test]
    fn distrusts_mismatched_or_odd_session_files() {
        let dir = tempfile::tempdir().unwrap();
        let body = format!(r#"{{"pid":333,"sessionId":"{SESSION}"}}"#);
        write_session(dir.path(), "444.json", &body);
        write_session(dir.path(), "notes.json", &body);
        write_session(dir.path(), "333.json.bak", &body);
        write_session(dir.path(), "555.json", "{not json");
        assert_eq!(session_pid(dir.path(), SESSION), None);
        assert_eq!(session_pid(&dir.path().join("missing"), SESSION), None);
    }

    #[cfg(unix)]
    #[test]
    fn ignores_symlinked_session_files() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::write(&real, format!(r#"{{"pid":666,"sessionId":"{SESSION}"}}"#)).unwrap();
        std::os::unix::fs::symlink(&real, dir.path().join("666.json")).unwrap();
        assert_eq!(session_pid(dir.path(), SESSION), None);
    }

    fn pane(pty_id: u32, pane_id: u32, tty: &str) -> PaneTty {
        PaneTty {
            pty_id,
            pane_id,
            owner: Arc::from("main"),
            tty: PathBuf::from(tty),
        }
    }

    fn fake_tty_dev(path: &Path) -> Option<u64> {
        path.to_str()?.strip_prefix("/dev/ttys")?.parse().ok()
    }

    fn resolve_with(request: &FocusRequest, panes: &[PaneTty], dir: Option<&Path>) -> Option<u32> {
        // pid N runs on /dev/ttys(N / 100); pid 1 has no terminal.
        let pid_dev = |pid: u32| (pid >= 100).then_some(u64::from(pid / 100));
        let probe = Probe {
            sessions_dir: dir,
            pid_tty_dev: &pid_dev,
            tty_dev: &fake_tty_dev,
        };
        resolve(request, panes, &probe).map(|p| p.pane_id)
    }

    #[test]
    fn resolves_tty_then_pid_then_session() {
        let panes = [
            pane(1, 10, "/dev/ttys001"),
            pane(2, 20, "/dev/ttys002"),
            pane(3, 30, "/dev/ttys003"),
        ];
        let dir = tempfile::tempdir().unwrap();
        write_session(
            dir.path(),
            "300.json",
            &format!(r#"{{"pid":300,"sessionId":"{SESSION}"}}"#),
        );

        let tty = |t: &str| normalize_tty(t);
        let req = |tty, pid, session: Option<&str>| FocusRequest {
            tty,
            pid,
            session: session.map(str::to_string),
        };

        assert_eq!(
            resolve_with(&req(tty("ttys002"), Some(100), None), &panes, None),
            Some(20)
        );
        assert_eq!(
            resolve_with(&req(tty("ttys009"), Some(100), None), &panes, None),
            Some(10)
        );
        assert_eq!(
            resolve_with(&req(None, Some(1), Some(SESSION)), &panes, Some(dir.path())),
            Some(30)
        );
        assert_eq!(
            resolve_with(&req(None, None, Some(SESSION)), &panes, None),
            None
        );
        assert_eq!(
            resolve_with(&req(tty("ttys009"), Some(900), None), &panes, None),
            None
        );
    }

    #[test]
    fn prefers_the_newest_terminal_on_a_reused_tty() {
        let panes = [pane(5, 50, "/dev/ttys004"), pane(9, 90, "/dev/ttys004")];
        let req = FocusRequest {
            tty: normalize_tty("ttys004"),
            ..Default::default()
        };
        assert_eq!(resolve_with(&req, &panes, None), Some(90));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_process_without_a_terminal_has_no_tty() {
        assert_eq!(pid_tty_dev(0x7fff_fff0), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_child_of_a_real_pty_resolves_to_its_tty() {
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};
        let pair = native_pty_system()
            .openpty(PtySize::default())
            .expect("open pty");
        let mut cmd = CommandBuilder::new("sleep");
        cmd.arg("30");
        let mut child = pair.slave.spawn_command(cmd).expect("spawn sleep");
        let tty = pair.master.tty_name().expect("master knows its tty");
        let pid = child.process_id().expect("child pid");
        let found = pid_tty_dev(pid);
        let _ = child.kill();
        let _ = child.wait();
        assert!(found.is_some());
        assert_eq!(found, tty_dev(&tty));
        assert_eq!(normalize_tty(tty.to_str().unwrap()), Some(tty));
    }
}
