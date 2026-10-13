//! Controller discovery watches directories without needing a device.

#[cfg(test)]
mod tests {
    use std::io;

    #[test]
    fn controller_node_removal_and_recreation_notify_discovery() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let node = directory.path().join("event0");
        std::fs::write(&node, [])?;
        let watcher = tehuti::controller::watch_input_directory(directory.path())?;
        let mut buffer = [0_u8; 4096];
        assert_eq!(
            rustix::io::read(&watcher, &mut buffer),
            Err(rustix::io::Errno::WOULDBLOCK)
        );
        std::fs::remove_file(&node)?;
        assert!(rustix::io::read(&watcher, &mut buffer)? > 0);
        std::fs::write(&node, [])?;
        assert!(rustix::io::read(&watcher, &mut buffer)? > 0);
        Ok(())
    }
}
