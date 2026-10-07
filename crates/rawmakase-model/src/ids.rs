//! Typed catalog row ids, so a collection's id can never be passed where a photo's
//! is meant. Each is the table's integer key, stored and serialized as that
//! integer. A leaf module: edit resolution and export, which sit below the
//! catalog, name photos with them too; `catalog` re-exports them.
use serde::{Deserialize, Serialize};

macro_rules! row_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub i64);
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
    /// A photo or one of its virtual copies (`photos.id`).
    PhotoId
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
    fn an_id_is_serialized_and_shown_as_its_integer() {
        let id = CollectionId(42);
        assert_eq!(serde_json::to_string(&id).unwrap(), "42");
        assert_eq!(serde_json::from_str::<CollectionId>("42").unwrap(), id);
        assert_eq!(id.to_string(), "42");
    }
}
