use std::io;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CoffeeBreak {
    /// A break of this many whole minutes.
    Timed(u32),
    #[default]
    Unlimited,
}

impl CoffeeBreak {
    pub fn label(self) -> String {
        match self {
            Self::Timed(minutes) => format_minutes(u64::from(minutes)),
            Self::Unlimited => "Until I stop".into(),
        }
    }

    pub fn seconds(self) -> Option<u64> {
        match self {
            Self::Timed(minutes) => Some(u64::from(minutes) * 60),
            Self::Unlimited => None,
        }
    }
}

/// "25 min", "1 h", "1 h 30": short enough for a segment, the tray and the status line.
pub fn format_minutes(minutes: u64) -> String {
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes:02}"),
    }
}

/// Owns exactly one child. Never finds or kills other caffeinate processes.
#[derive(Default)]
pub struct Caffeine {
    child: Option<Child>,
    deadline: Option<Instant>,
}

impl Caffeine {
    pub fn active(&self) -> bool {
        self.child.is_some()
    }

    pub fn start(&mut self, coffee_break: CoffeeBreak) -> io::Result<()> {
        self.refresh()?;
        if self.active() {
            return Ok(());
        }
        let mut command = Command::new("/usr/bin/caffeinate");
        // -w releases assertions even if this app is force-quit; -d and -i keep
        // the display and idle system awake, on battery as well as AC power.
        command.args(["-di", "-w", &std::process::id().to_string()]);
        if let Some(seconds) = coffee_break.seconds() {
            command.args(["-t", &seconds.to_string()]);
        }
        self.child = Some(
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?,
        );
        self.deadline = coffee_break
            .seconds()
            .map(|seconds| Instant::now() + Duration::from_secs(seconds));
        Ok(())
    }

    pub fn stop(&mut self) -> io::Result<()> {
        if let Some(child) = self.child.as_mut() {
            // Reap an already exited child before trying to signal it.
            if child.try_wait()?.is_none() {
                child.kill()?;
                child.wait()?;
            }
        }
        self.child = None;
        self.deadline = None;
        Ok(())
    }

    /// Returns true when an assertion ended, including a timed break or external kill.
    pub fn refresh(&mut self) -> io::Result<bool> {
        let exited = match self.child.as_mut() {
            Some(child) => child.try_wait()?.is_some(),
            None => false,
        };
        if exited {
            self.child = None;
            self.deadline = None;
        }
        Ok(exited)
    }

    pub fn minutes_left(&self) -> Option<u64> {
        self.deadline.map(|deadline| {
            deadline
                .saturating_duration_since(Instant::now())
                .as_secs()
                .div_ceil(60)
        })
    }
}

impl Drop for Caffeine {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod format_tests {
    use super::*;

    #[test]
    fn durations_read_as_minutes_then_hours() {
        assert_eq!(format_minutes(1), "1 min");
        assert_eq!(format_minutes(25), "25 min");
        assert_eq!(format_minutes(60), "1 h");
        assert_eq!(format_minutes(90), "1 h 30");
        assert_eq!(format_minutes(65), "1 h 05");
        assert_eq!(format_minutes(1440), "24 h");
        assert_eq!(CoffeeBreak::Timed(40).label(), "40 min");
        assert_eq!(CoffeeBreak::Unlimited.label(), "Until I stop");
        assert_eq!(CoffeeBreak::Timed(40).seconds(), Some(2400));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn start_is_idempotent_and_stop_only_reaps_our_child() {
        let mut other = Command::new("/usr/bin/caffeinate")
            .args(["-i", "-t", "10"])
            .spawn()
            .unwrap();
        let mut caffeine = Caffeine::default();
        caffeine.start(CoffeeBreak::Timed(25)).unwrap();
        let pid = caffeine.child.as_ref().unwrap().id();
        caffeine.start(CoffeeBreak::Timed(60)).unwrap();
        assert_eq!(caffeine.child.as_ref().unwrap().id(), pid);
        assert_eq!(caffeine.minutes_left(), Some(25));
        caffeine.stop().unwrap();
        caffeine.stop().unwrap();
        assert!(!caffeine.active());
        assert!(other.try_wait().unwrap().is_none());
        other.kill().unwrap();
        other.wait().unwrap();
    }

    #[test]
    fn an_externally_killed_child_can_be_started_again() {
        let mut caffeine = Caffeine::default();
        caffeine.start(CoffeeBreak::Unlimited).unwrap();
        caffeine.child.as_mut().unwrap().kill().unwrap();
        caffeine.child.as_mut().unwrap().wait().unwrap();
        assert!(caffeine.refresh().unwrap());
        assert!(!caffeine.active());
        caffeine.start(CoffeeBreak::Timed(60)).unwrap();
        assert!(caffeine.active());
    }

    #[test]
    fn dropping_the_owner_terminates_the_child() {
        let mut caffeine = Caffeine::default();
        caffeine.start(CoffeeBreak::Unlimited).unwrap();
        // Retain the Child only to observe cleanup after the owner drops.
        let child = caffeine.child.take().unwrap();
        let pid = child.id();
        caffeine.child = Some(child);
        drop(caffeine);
        let status = Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "pid="])
            .output()
            .unwrap();
        assert!(!status.status.success());
    }
}
