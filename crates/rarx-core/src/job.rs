use std::sync::atomic::{AtomicBool, Ordering};

use crate::{Error, Result};

/// Shared cancel/pause switches for a running job.
///
/// Workers call [`JobControl::checkpoint`] between units of work; front-ends flip the
/// switches from their own threads.
#[derive(Debug, Default)]
pub struct JobControl {
    cancelled: AtomicBool,
    paused: AtomicBool,
}

impl JobControl {
    /// Requests cancellation. Irreversible for this job.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Whether cancellation was requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Pauses or resumes the job.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    /// Whether the job is paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Returns [`Error::Cancelled`] once cancellation was requested.
    ///
    /// # Errors
    /// [`Error::Cancelled`] if [`JobControl::cancel`] has been called.
    pub fn checkpoint(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_fails_after_cancel() {
        let ctl = JobControl::default();
        assert!(ctl.checkpoint().is_ok());
        ctl.cancel();
        assert!(matches!(ctl.checkpoint(), Err(Error::Cancelled)));
    }

    #[test]
    fn pause_toggles() {
        let ctl = JobControl::default();
        ctl.set_paused(true);
        assert!(ctl.is_paused());
        ctl.set_paused(false);
        assert!(!ctl.is_paused());
    }
}
