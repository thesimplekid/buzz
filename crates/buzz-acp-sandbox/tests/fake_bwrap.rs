#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn wrapper_invokes_bwrap_and_preserves_real_acp_args() {
    let temp = temp_dir("buzz-acp-sandbox-test");
    let fake_bwrap = temp.join("fake-bwrap");
    let fake_acp = temp.join("buzz-acp");
    let bwrap_log = temp.join("bwrap.log");
    let acp_log = temp.join("acp.log");
    let root = temp.join("root");

    write_executable(
        &fake_bwrap,
        r#"#!/bin/sh
printf '%s\n' "$@" > "$BUZZ_FAKE_BWRAP_LOG"
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--" ]; then
    shift
    break
  fi
  shift
done
cmd="$1"
shift
if [ ! -x "$cmd" ] && [ -n "$BUZZ_FAKE_REAL_ACP" ]; then
  cmd="$BUZZ_FAKE_REAL_ACP"
fi
exec "$cmd" "$@"
"#,
    );
    write_executable(
        &fake_acp,
        r#"#!/bin/sh
printf '%s\n' "$@" > "$BUZZ_FAKE_ACP_LOG"
exit 23
"#,
    );

    let status = Command::new(env!("CARGO_BIN_EXE_buzz-acp-sandbox"))
        .arg("--respond-to")
        .arg("anyone")
        .env("BUZZ_SANDBOX_MODE", "required")
        .env("BUZZ_SANDBOX_ROOT", &root)
        .env("BUZZ_SANDBOX_ID", "test-agent")
        .env("BUZZ_BWRAP", &fake_bwrap)
        .env("BUZZ_REAL_ACP", &fake_acp)
        .env("BUZZ_FAKE_BWRAP_LOG", &bwrap_log)
        .env("BUZZ_FAKE_ACP_LOG", &acp_log)
        .env("BUZZ_FAKE_REAL_ACP", &fake_acp)
        .status()
        .expect("run wrapper");

    assert_eq!(status.code(), Some(23));

    let bwrap_args = fs::read_to_string(&bwrap_log).expect("bwrap log");
    assert!(bwrap_args.contains("--unshare-pid\n"));
    assert!(bwrap_args.contains("--bind\n"));
    assert!(bwrap_args.contains("/home/buzz\n"));
    assert!(bwrap_args.contains("--\n"));
    assert!(bwrap_args.contains("/run/buzz/bin/buzz-acp\n"));

    let acp_args = fs::read_to_string(&acp_log).expect("acp log");
    assert_eq!(acp_args, "--respond-to\nanyone\n");
    assert!(root.join("test-agent/home").is_dir());
    assert!(root.join("test-agent/tmp").is_dir());

    let _ = fs::remove_dir_all(temp);
}

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write executable");
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod executable");
}
