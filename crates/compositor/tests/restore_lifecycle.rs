//! Restored applications survive the launch helper and its process group.

use std::{os::unix::process::CommandExt as _, process::Command};
use tehuti::launch::Launch;

#[cfg(test)]
mod tests {
use super::*;

#[test]
fn restored_application_outlives_its_helper() {
    const CHILD: &str = "TEHUTI_RESTORE_LIFETIME_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let socket = std::env::var_os("TEHUTI_RESTORE_LIFETIME_SOCKET").unwrap();
        let mut stream = std::os::unix::net::UnixStream::connect(socket).unwrap();
        use std::io::{Read as _, Write as _};
        stream.write_all(&rustix::process::getpgrp().as_raw_nonzero().get().to_ne_bytes()).unwrap();
        let mut release = [0];
        stream.read_exact(&mut release).unwrap();
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("child.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let executable = std::env::current_exe().unwrap();
    let mut helper = Command::new("/bin/sh")
        .process_group(0)
        .args(["-c", "\"$@\" & wait", "restore-helper"])
        .arg(&executable)
        .args(["--exact", "tests::restore_launch_probe", "--nocapture"])
        .env("TEHUTI_RESTORE_LIFETIME_SOCKET", &socket)
        .spawn()
        .unwrap();
    listener.set_nonblocking(true).unwrap();
    let start = std::time::Instant::now();
    let mut connection = loop {
        if let Ok((stream, _)) = listener.accept() {
            break stream;
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(10));
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    use std::io::{Read as _, Write as _};
    connection.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    let mut ready = [0; 4];
    connection.read_exact(&mut ready).unwrap();
    let helper_pid = i32::try_from(helper.id()).unwrap();
    assert!(helper.wait().unwrap().success());
    let independent = i32::from_ne_bytes(ready) != helper_pid;
    connection.write_all(&[1]).unwrap();
    let mut output = Vec::new();
    connection.read_to_end(&mut output).unwrap();
    assert!(independent, "restored application must leave the helper process group");
}

#[test]
fn restore_launch_probe() {
    let Some(socket) = std::env::var_os("TEHUTI_RESTORE_LIFETIME_SOCKET") else {
        return;
    };
    let _child = Launch::new(std::env::current_exe().unwrap(), "Restored application")
        .arg("--exact")
        .arg("tests::restored_application_outlives_its_helper")
        .with_env("TEHUTI_RESTORE_LIFETIME_CHILD", "1")
        .with_env("TEHUTI_RESTORE_LIFETIME_SOCKET", socket)
        .command()
        .spawn()
        .unwrap();

}

}
