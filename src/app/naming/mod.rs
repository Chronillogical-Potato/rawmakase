//! What names files by a template, wherever files are named: the Filename
//! Template Editor (`editor`) and the background read of what names need
//! from the photos' files (`read`). Export owns one of each for its dialog;
//! neither knows who opened it.
pub(super) mod editor;
pub(super) mod read;
