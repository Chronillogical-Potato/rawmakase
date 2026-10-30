//! File checksums of the photos, read in the background (see
//! `catalog_session::checksums`) for finding photos moved or renamed outside
//! RAWmakase.
use super::Library;

impl Library {
    pub(super) fn start_checksums(&mut self) {
        if self.reads_suspended {
            return;
        }
        let availability = &self.availability;
        self.session
            .start_checksums(|path| availability.is_available(path));
    }
    /// Saves the checksums read so far.
    pub(super) fn poll_checksums(&mut self) {
        let polled = self.session.poll_checksums();
        if let Some(e) = polled.error {
            // Read again after the next online check.
            self.messages
                .warn(format!("Checksums could not be saved: {e}"));
        }
        if polled.start_again {
            self.start_checksums();
        }
    }
}
