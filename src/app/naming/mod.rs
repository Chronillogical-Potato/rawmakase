//! What names files by a template, wherever files are named: the menu of
//! templates (`menu`), the Filename Template Editor (`editor`) and the
//! background read of what names need from the photos' files (`read`).
//! Export and Rename Photos each own an editor and a read for their dialog;
//! none of these knows who opened it.
pub(super) mod editor;
pub(super) mod menu;
pub(super) mod read;
