//! One undo sequence across Library and Develop, as Lightroom has: Cmd+Z
//! reverses the latest command wherever it was made, after returning to the
//! place it was made in. The History panel keeps its own steps and imported
//! Lightroom history; this log only decides what Cmd+Z and Cmd+Shift+Z do.
//! It lives in memory and is cleared when another catalog opens.
use super::Editor;
use super::history::{Recorded, Step};
use super::library::MetadataCommand;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

/// Commands kept, as many as History keeps steps.
const LIMIT: usize = 100;

/// A number that orders commands made in the same frame.
pub(super) fn sequence() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Command {
    /// Rating, flag or label of one or more photos. `develop` is the photo
    /// open in Develop when it was made there; it is undone there.
    Metadata {
        change: Box<MetadataCommand>,
        develop: Option<i64>,
    },
    /// A Develop step or History click on `photo` (`None`: a file outside
    /// the catalog), in the History identified by `history`.
    Develop {
        photo: Option<i64>,
        history: u64,
        change: Box<Recorded>,
    },
}

#[derive(Default)]
pub(super) struct UndoLog {
    undo: VecDeque<Command>,
    redo: Vec<Command>,
}
impl UndoLog {
    pub(super) fn push(&mut self, command: Command) {
        if self.undo.len() == LIMIT {
            self.undo.pop_front();
        }
        self.undo.push_back(command);
        self.redo.clear();
    }
    pub(super) fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
    #[cfg(test)]
    pub(super) fn len(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Direction {
    Undo,
    Redo,
}

impl Editor {
    /// Moves the changes Library and Develop made into the log, in the order
    /// they were made.
    pub(super) fn sync_undo(&mut self) {
        let photo = self.document.catalog_photo;
        let history = self.document.history.id();
        let mut commands: Vec<(u64, Command)> = self
            .document
            .history
            .take_recorded()
            .into_iter()
            .map(|change| {
                (
                    change.sequence,
                    Command::Develop {
                        photo,
                        history,
                        change: Box::new(change),
                    },
                )
            })
            .collect();
        if let Some(library) = &mut self.library {
            let develop = if self.library_mode { None } else { photo };
            commands.extend(library.take_done().into_iter().map(|change| {
                (
                    change.sequence,
                    Command::Metadata {
                        change: Box::new(change),
                        develop,
                    },
                )
            }));
        }
        commands.sort_by_key(|(sequence, _)| *sequence);
        for (_, command) in commands {
            self.undo_log.push(command);
        }
    }
    pub(super) fn undo(&mut self) {
        self.sync_undo();
        if let Some(command) = self.undo_log.undo.pop_back() {
            if self.apply(&command, Direction::Undo) {
                self.undo_log.redo.push(command);
            } else {
                self.undo_log.undo.push_back(command);
            }
        }
    }
    pub(super) fn redo(&mut self) {
        self.sync_undo();
        if let Some(command) = self.undo_log.redo.pop() {
            if self.apply(&command, Direction::Redo) {
                self.undo_log.undo.push_back(command);
            } else {
                self.undo_log.redo.push(command);
            }
        }
    }
    /// Returns to where `command` was made and sets its state from before
    /// (undo) or after (redo). False when nothing could be written, so the
    /// command stays where it was.
    fn apply(&mut self, command: &Command, direction: Direction) -> bool {
        let verb = match direction {
            Direction::Undo => "Undo",
            Direction::Redo => "Redo",
        };
        match command {
            Command::Metadata { change, develop } => {
                let (values, place) = match direction {
                    Direction::Undo => (&change.before, &change.place_before),
                    Direction::Redo => (&change.after, &change.place_after),
                };
                let Some(library) = &mut self.library else {
                    return false;
                };
                if let Err(e) = library.set_metadata(values) {
                    self.status = format!("{verb} failed: {e}");
                    return false;
                }
                match develop {
                    Some(photo) => self.show_in_develop(*photo),
                    None => {
                        if !self.library_mode && self.flush() {
                            self.library_mode = true;
                        }
                        if let Some(library) = &mut self.library {
                            library.go_to_place(place);
                        }
                    }
                }
                self.status = format!("{verb} {}", change.summary);
                true
            }
            Command::Develop {
                photo,
                history,
                change,
            } => {
                let (target, at) = match direction {
                    Direction::Undo => (&change.before, change.at_before),
                    Direction::Redo => (&change.after, change.at_after),
                };
                let open = self.document.history.id() == *history
                    || (photo.is_some()
                        && self.document.catalog_photo == *photo
                        && self.document.path.is_some());
                if open {
                    self.library_mode = false;
                    let step = Step::new(verb, "");
                    self.document
                        .history
                        .restore(at, target, &mut self.document.recipe, step);
                    // Not a change of its own for the log.
                    self.document.history.take_recorded();
                    self.document.save.mark_changed();
                    self.ensure_upright();
                    self.schedule();
                    self.status = format!("{verb} in Develop");
                    return true;
                }
                // The photo was closed since: save the state and open it.
                let Some(id) = *photo else {
                    self.status = format!("{verb}: that photo is no longer open");
                    return true;
                };
                let Some(library) = &self.library else {
                    return false;
                };
                let Some(path) = library.photo(id).map(|p| p.path.clone()) else {
                    self.status = format!("{verb}: that photo is no longer in the catalog");
                    return true;
                };
                let saved = library.catalog.load_edit(id, &path).and_then(|edit| {
                    let export = edit.map(|e| e.export).unwrap_or_default();
                    library.catalog.save_edit(id, &path, target, &export)
                });
                if let Err(e) = saved {
                    self.status = format!("{verb} failed: {e}");
                    return false;
                }
                self.develop_catalog_photo(id);
                true
            }
        }
    }
    /// Shows `photo` in Develop, keeping its edit when it is already open.
    fn show_in_develop(&mut self, photo: i64) {
        if self.document.catalog_photo == Some(photo) && self.document.path.is_some() {
            self.library_mode = false;
            if let Some(library) = &mut self.library {
                library.make_active(photo);
            }
        } else {
            self.develop_catalog_photo(photo);
        }
    }
}
