use std::io;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CoffeeBreak {
    Short,
    Hour,
    #[default]
    Unlimited,
}

impl CoffeeBreak {
    pub const ALL: [Self; 3] = [Self::Short, Self::Hour, Self::Unlimited];

    pub fn label(self) -> &'static str {
        match self {
            Self::Short => "25 min",
            Self::Hour => "1 hour",
            Self::Unlimited => "Until I stop",
        }
    }

    pub fn seconds(self) -> Option<u64> {
        match self {
            Self::Short => Some(25 * 60),
            Self::Hour => Some(60 * 60),
            Self::Unlimited => None,
        }
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
        caffeine.start(CoffeeBreak::Short).unwrap();
        let pid = caffeine.child.as_ref().unwrap().id();
        caffeine.start(CoffeeBreak::Hour).unwrap();
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
        caffeine.start(CoffeeBreak::Hour).unwrap();
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
