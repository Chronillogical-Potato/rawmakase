//! Typed row ids, so a collection's id can never be passed where a photo's is
//! meant. Each is the table's integer key, stored and serialized as that integer.
use rusqlite::types::{FromSql, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::{Deserialize, Serialize};

macro_rules! row_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub i64);
        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                self.0.to_sql()
            }
        }
        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                i64::column_result(value).map(Self)
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

row_id!(
    /// A collection or collection set (`collections.id`).
    CollectionId
);
row_id!(
    /// A folder of photos (`folders.id`).
    FolderId
);
row_id!(
    /// A root folder, as a catalog records it (`roots.id`); its folders are paths
    /// under it.
    RootId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_stored_and_serialized_as_its_integer() -> rusqlite::Result<()> {
        let db = rusqlite::Connection::open_in_memory()?;
        let id: CollectionId = db.query_row("SELECT ?1", [CollectionId(42)], |r| r.get(0))?;
        assert_eq!(id, CollectionId(42));
        assert_eq!(serde_json::to_string(&id).unwrap(), "42");
        assert_eq!(id.to_string(), "42");
        Ok(())
    }
}
