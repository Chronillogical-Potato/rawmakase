//! The edit of the photo open in Develop, apart from the window that shows it:
//! its History and whether it still needs saving.
pub mod history;
pub mod save_state;

/// A number that orders the changes made in the same frame, edits and catalog
/// commands alike.
pub fn sequence() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
