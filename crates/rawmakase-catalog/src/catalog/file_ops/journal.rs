//! The journal of file operations: the `file_operations` and
//! `file_operation_members` rows, written and read only here. Each disk step
//! is journalled before the next one starts, so whatever a crash leaves is
//! decided by [`super::recover`] from the journal and the disk, never
//! guessed.
use super::super::PhotoId;
use super::super::db::{Reads, Write, sql};
use super::super::value::row;
use super::folder::FolderState;
use super::preconditions::Preconditions;
use crate::storage::Identity;
use crate::storage::fsops::FileId;
use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};

/// Maps each variant to the one text the journal stores for it, and back.
macro_rules! stored {
    ($(#[$meta:meta])* $name:ident { $($(#[$v_meta:meta])* $variant:ident = $text:literal),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub(in crate::catalog) enum $name {
            $($(#[$v_meta])* $variant),*
        }
        impl $name {
            #[cfg(test)]
            pub(super) const ALL: &[Self] = &[$(Self::$variant),*];
            pub(super) fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),*
                }
            }
        }
        impl std::str::FromStr for $name {
            type Err = anyhow::Error;
            fn from_str(text: &str) -> Result<Self> {
                match text {
                    $($text => Ok(Self::$variant),)*
                    other => Err(anyhow!(concat!("unknown ", stringify!($name), " {:?}"), other)),
                }
            }
        }
    };
}

stored! {
    /// What an item does.
    Kind {
        /// Its photos leave the catalog; nothing on disk changes.
        RemoveFromCatalog = "remove_from_catalog",
        /// Its photo's file and companions move to another folder.
        MovePhotos = "move_photos",
        /// Its photo's file and companions go to the Trash, then its rows.
        TrashPhotos = "trash_photos",
        /// A folder is made, on disk and then in the catalog.
        CreateFolder = "create_folder",
    }
}

stored! {
    /// Where an item stands.
    ItemState {
        /// Journalled; its steps may have begun.
        Planned = "planned",
        /// Being put back: never completed forward again.
        Restoring = "restoring",
        /// Finished with nothing changed, on disk or in the catalog.
        NoChange = "no_change",
        /// Unfinished, for the user to decide.
        NeedsRecovery = "needs_recovery",
        /// Left as it is by the user; never finished later.
        Kept = "kept",
        Done = "done",
    }
}

stored! {
    /// Where a member stands.
    MemberState {
        Planned = "planned",
        Moved = "moved",
        Trashed = "trashed",
        /// A folder made, at its destination: the name the disk gave it.
        Created = "created",
        /// Put back where it was.
        Restored = "restored",
    }
}

/// A journalled item's row id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OperationId(pub(super) i64);

/// A file or folder an item will touch, as planned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NewMember {
    pub source: PathBuf,
    /// Where it goes; `None` for the Trash.
    pub destination: Option<PathBuf>,
    pub id: FileId,
    pub identity: Identity,
}

/// A journalled member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Member {
    pub ordinal: i64,
    pub source: PathBuf,
    pub destination: Option<PathBuf>,
    pub id: Option<FileId>,
    pub identity: Option<Identity>,
    pub state: MemberState,
    /// Where the Trash put it.
    pub receipt: Option<PathBuf>,
}

impl Member {
    /// Where the member is once its step is done: its destination, or where
    /// the Trash put it.
    pub fn after(&self) -> Option<&Path> {
        self.destination.as_deref().or(self.receipt.as_deref())
    }
}

/// What a journalled item is about, with what must still hold of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Subject {
    /// A family of photos, or a lone virtual copy.
    Photos(Preconditions),
    /// A folder it makes.
    Folder(FolderState),
}

/// A journalled item.
#[derive(Clone, Debug)]
pub(super) struct Item {
    pub id: OperationId,
    /// The computer whose files it changes, the only one to decide it.
    pub computer: String,
    pub kind: Kind,
    pub state: ItemState,
    pub subject: Subject,
    pub members: Vec<Member>,
    pub error: Option<String>,
}

impl Item {
    /// The catalog photos it concerns; none for a folder.
    pub fn photos(&self) -> &[PhotoId] {
        match &self.subject {
            Subject::Photos(pre) => &pre.rows,
            Subject::Folder(_) => &[],
        }
    }
}

/// A path as the journal stores it. Catalog paths are text, so a path that
/// isn't can't be journalled, and is never moved.
pub(super) fn text(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("{} is not a name RAWmakase can record", path.display()))
}

/// Journals a new item of `batch` in `state`, with its members planned.
#[allow(clippy::too_many_arguments, reason = "one row's columns")]
pub(super) fn insert(
    w: &mut Write<'_>,
    batch: &str,
    computer: &str,
    kind: Kind,
    state: ItemState,
    preconditions: &Preconditions,
    members: &[NewMember],
    error: Option<&str>,
) -> Result<OperationId> {
    let id: i64 = w.insert_returning_id(
        sql!(
            "INSERT INTO file_operations(batch, computer, kind, state, preconditions, created_at, error)
             VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ),
        &[
            &batch,
            &computer,
            &kind.as_str(),
            &state.as_str(),
            &serde_json::to_string(preconditions)?,
            &crate::time::now_text(),
            &error,
        ],
    )?;
    insert_members(w, id, members)?;
    Ok(OperationId(id))
}

fn insert_members(w: &mut Write<'_>, id: i64, members: &[NewMember]) -> Result<()> {
    for (ordinal, m) in members.iter().enumerate() {
        let destination = m.destination.as_deref().map(text).transpose()?;
        w.execute(
            sql!(
                "INSERT INTO file_operation_members(operation, ordinal, source, destination,
                    file_id, identity, state)
                 VALUES (?, ?, ?, ?, ?, ?, ?)"
            ),
            &[
                &id,
                &(ordinal as i64),
                &text(&m.source)?,
                &destination,
                &m.id.to_string(),
                &serde_json::to_string(&m.identity)?,
                &MemberState::Planned.as_str(),
            ],
        )?;
    }
    Ok(())
}

/// Journals a new item of `batch` making folder `path`, in `state`: its one
/// member has no file object yet, as the folder isn't made.
pub(super) fn insert_folder(
    w: &mut Write<'_>,
    batch: &str,
    computer: &str,
    state: ItemState,
    folder: &FolderState,
    path: &Path,
    error: Option<&str>,
) -> Result<OperationId> {
    let id: i64 = w.insert_returning_id(
        sql!(
            "INSERT INTO file_operations(batch, computer, kind, state, preconditions, created_at, error)
             VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ),
        &[
            &batch,
            &computer,
            &Kind::CreateFolder.as_str(),
            &state.as_str(),
            &serde_json::to_string(folder)?,
            &crate::time::now_text(),
            &error,
        ],
    )?;
    w.execute(
        sql!(
            "INSERT INTO file_operation_members(operation, ordinal, source, state)
             VALUES (?, 0, ?, ?)"
        ),
        &[&id, &text(path)?, &MemberState::Planned.as_str()],
    )?;
    Ok(OperationId(id))
}

/// Records the folder item `id` made: its file object, and the name the
/// disk gave it at `made`.
pub(super) fn set_created(
    w: &mut Write<'_>,
    id: OperationId,
    file: FileId,
    made: &Path,
) -> Result<()> {
    let changed = w.execute(
        sql!(
            "UPDATE file_operation_members SET state=?, file_id=?, destination=?
             WHERE operation=? AND ordinal=0"
        ),
        &[
            &MemberState::Created.as_str(),
            &file.to_string(),
            &text(made)?,
            &id.0,
        ],
    )?;
    anyhow::ensure!(changed == 1, "unknown file operation member");
    Ok(())
}

/// Replaces folder item `id`'s state, as it is once the catalog has it.
pub(super) fn set_folder(w: &mut Write<'_>, id: OperationId, folder: &FolderState) -> Result<()> {
    let changed = w.execute(
        sql!("UPDATE file_operations SET preconditions=? WHERE id=? AND kind=?"),
        &[
            &serde_json::to_string(folder)?,
            &id.0,
            &Kind::CreateFolder.as_str(),
        ],
    )?;
    anyhow::ensure!(changed == 1, "unknown folder operation");
    Ok(())
}

/// Sets item `id`'s state, and its error when one is given.
pub(super) fn set_state(
    w: &mut Write<'_>,
    id: OperationId,
    state: ItemState,
    error: Option<&str>,
) -> Result<()> {
    let changed = w.execute(
        sql!("UPDATE file_operations SET state=?, error=COALESCE(?, error) WHERE id=?"),
        &[&state.as_str(), &error, &id.0],
    )?;
    anyhow::ensure!(changed == 1, "unknown file operation");
    Ok(())
}

/// Sets member `ordinal` of item `id`'s state, with where the Trash put it.
pub(super) fn set_member(
    w: &mut Write<'_>,
    id: OperationId,
    ordinal: i64,
    state: MemberState,
    receipt: Option<&Path>,
) -> Result<()> {
    let receipt = receipt.map(text).transpose()?;
    let changed = w.execute(
        sql!(
            "UPDATE file_operation_members SET state=?, receipt=COALESCE(?, receipt)
             WHERE operation=? AND ordinal=?"
        ),
        &[&state.as_str(), &receipt, &id.0, &ordinal],
    )?;
    anyhow::ensure!(changed == 1, "unknown file operation member");
    Ok(())
}

row! {
    struct ItemRow {
        id: i64,
        computer: String,
        kind: String,
        state: String,
        preconditions: String,
        error: Option<String>,
    }
}
row! {
    struct MemberRow {
        ordinal: i64,
        source: String,
        destination: Option<String>,
        file_id: Option<String>,
        identity: Option<String>,
        state: String,
        receipt: Option<String>,
    }
}

fn read(db: &impl Reads, row: ItemRow) -> Result<Item> {
    let members: Vec<MemberRow> = db.read(
        sql!(
            "SELECT ordinal, source, destination, file_id, identity, state, receipt
             FROM file_operation_members WHERE operation=? ORDER BY ordinal"
        ),
        &[&row.id],
    )?;
    let members = members
        .into_iter()
        .map(|m| {
            Ok(Member {
                ordinal: m.ordinal,
                source: m.source.into(),
                destination: m.destination.map(PathBuf::from),
                id: m.file_id.map(|t| t.parse()).transpose()?,
                identity: m.identity.map(|t| serde_json::from_str(&t)).transpose()?,
                state: m.state.parse()?,
                receipt: m.receipt.map(PathBuf::from),
            })
        })
        .collect::<Result<_>>()?;
    let kind: Kind = row.kind.parse()?;
    let subject = match kind {
        Kind::CreateFolder => Subject::Folder(serde_json::from_str(&row.preconditions)?),
        _ => Subject::Photos(serde_json::from_str(&row.preconditions)?),
    };
    Ok(Item {
        id: OperationId(row.id),
        computer: row.computer,
        kind,
        state: row.state.parse()?,
        subject,
        members,
        error: row.error,
    })
}

/// Item `id`, if it is journalled.
pub(super) fn item(db: &impl Reads, id: OperationId) -> Result<Option<Item>> {
    let row: Option<ItemRow> = db.read_optional(
        sql!(
            "SELECT id, computer, kind, state, preconditions, error FROM file_operations WHERE id=?"
        ),
        &[&id.0],
    )?;
    row.map(|row| read(db, row)).transpose()
}

/// Every item of `batch`, in the order they were journalled.
pub(super) fn of_batch(db: &impl Reads, batch: &str) -> Result<Vec<Item>> {
    let rows: Vec<ItemRow> = db.read(
        sql!(
            "SELECT id, computer, kind, state, preconditions, error FROM file_operations
             WHERE batch=? ORDER BY id"
        ),
        &[&batch],
    )?;
    rows.into_iter().map(|row| read(db, row)).collect()
}

/// Every item of `computer` still to be decided, oldest first.
pub(super) fn unresolved(db: &impl Reads, computer: &str) -> Result<Vec<Item>> {
    let rows: Vec<ItemRow> = db.read(
        sql!(
            "SELECT id, computer, kind, state, preconditions, error FROM file_operations
             WHERE computer=? AND state IN ('planned', 'restoring', 'needs_recovery')
             ORDER BY id"
        ),
        &[&computer],
    )?;
    rows.into_iter().map(|row| read(db, row)).collect()
}

/// Every item still to be decided, whichever computer's.
pub(super) fn unresolved_anywhere(db: &impl Reads) -> Result<Vec<Item>> {
    let rows: Vec<ItemRow> = db.read(
        sql!(
            "SELECT id, computer, kind, state, preconditions, error FROM file_operations
             WHERE state IN ('planned', 'restoring', 'needs_recovery')
             ORDER BY id"
        ),
        &[],
    )?;
    rows.into_iter().map(|row| read(db, row)).collect()
}

/// Forgets the finished items journalled before `before` (a time as
/// [`crate::time::utc_text`] writes it). Returns how many.
pub(super) fn prune(w: &mut Write<'_>, before: &str) -> Result<usize> {
    w.execute(
        sql!(
            "DELETE FROM file_operations
             WHERE state IN ('done', 'no_change', 'kept') AND created_at < ?"
        ),
        &[&before],
    )
}
