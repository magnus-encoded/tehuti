//! Bounded child shutdown and session termination signals.

use core::{sync::atomic::AtomicBool, time::Duration};

use std::{process::Child, sync::Arc, time::Instant};

#[derive(Debug)]
pub struct Termination {
    flag: Arc<AtomicBool>,
    registrations: Vec<signal_hook::SigId>,
}

impl Termination {
    /// Registers the signals that end a login session.
    ///
    /// # Errors
    /// Signal registration failed.
    pub fn register() -> std::io::Result<Self> {
        let mut termination = Self {
            flag: Arc::new(AtomicBool::new(false)),
            registrations: Vec::new(),
        };
        for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
            termination
                .registrations
                .push(signal_hook::flag::register(signal, termination.flag.clone())?);
        }
        Ok(termination)
    }

    pub fn stop_if_requested(&self, signal: &smithay::reexports::calloop::LoopSignal) {
        if self.flag.load(core::sync::atomic::Ordering::Relaxed) {
            signal.stop();
        }
    }
}

impl Drop for Termination {
    fn drop(&mut self) {
        for registration in self.registrations.drain(..) {
            let _removed = signal_hook::low_level::unregister(registration);
        }
    }
}

pub fn terminate(child: &mut Child, process_group: bool) {
    if !process_group && matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    let Some(pid) = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return;
    };
    let send = |signal| {
        let result = if process_group {
            rustix::process::kill_process_group(pid, signal)
        } else {
            rustix::process::kill_process(pid, signal)
        };
        if let Err(error) = result {
            tracing::debug!(%error, "child signal failed");
        }
    };
    send(rustix::process::Signal::TERM);
    let started = Instant::now();
    loop {
        let exited = matches!(child.try_wait(), Ok(Some(_)));
        if exited && !process_group {
            return;
        }
        if started.elapsed() >= Duration::from_millis(200) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    send(rustix::process::Signal::KILL);
    if let Err(error) = child.wait() {
        tracing::warn!(%error, "child reap failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn sigterm_stops_the_loop_cleanly() {
        let termination = Termination::register().unwrap();
        let mut event_loop = smithay::reexports::calloop::EventLoop::<()>::try_new().unwrap();
        let signal = event_loop.get_signal();
        signal_hook::low_level::raise(signal_hook::consts::SIGTERM).unwrap();
        let mut iterations = 0;
        event_loop
            .run(Some(Duration::ZERO), &mut (), |()| {
                iterations += 1;
                termination.stop_if_requested(&signal);
            })
            .unwrap();
        assert_eq!(iterations, 1);
    }

    #[test]
    fn children_receive_term_and_stubborn_children_receive_kill() {
        for (handler, expected) in [("raise SystemExit(0)", Some(0)), ("pass", None)] {
            let directory = tempfile::tempdir().unwrap();
            let ready = directory.path().join("ready");
            let mut child = std::process::Command::new("python3").arg("-c").arg(format!(
                "import signal,time,pathlib\ndef stop(*args):\n {handler}\nsignal.signal(signal.SIGTERM,stop)\npathlib.Path({:?}).touch()\nwhile True: time.sleep(.01)", ready.to_str().unwrap()
            )).spawn().unwrap();
            let started = Instant::now();
            while !ready.exists() {
                assert!(started.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(10));
            }
            terminate(&mut child, false);
            let status = child.wait().unwrap();
            assert_eq!(status.code(), expected);
            if expected.is_none() {
                assert_eq!(status.signal(), Some(signal_hook::consts::SIGKILL));
            }
        }
    }
}
