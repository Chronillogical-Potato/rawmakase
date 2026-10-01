//! What the Library shows: the source (a folder scope or collection), the filter
//! bar's search, flag, rating and label, the offline filter and the sort order.
use crate::catalog::Photo;
use std::{collections::HashSet, path::Path};

pub(super) struct Filters {
    /// Folder ids in the chosen folder and its subfolders; None is All Photographs.
    pub folder_scope: Option<HashSet<i64>>,
    pub collection: Option<i64>,
    /// The chosen collection's photos.
    pub members: HashSet<i64>,
    pub query: String,
    /// Minimum rating.
    pub rating: i32,
    /// 1 picked, 0 unflagged, -1 rejected; 2 is every flag.
    pub flag: i32,
    pub label_filter: Option<String>,
    pub only_missing: bool,
    pub reverse: bool,
}
impl Default for Filters {
    fn default() -> Self {
        Self {
            folder_scope: None,
            collection: None,
            members: HashSet::new(),
            query: String::new(),
            rating: 0,
            flag: 2,
            label_filter: None,
            only_missing: false,
            reverse: false,
        }
    }
}
impl Filters {
    /// Indices into `photos` of the ones shown, in display order.
    pub(super) fn visible(
        &self,
        photos: &[Photo],
        available: impl Fn(&Path) -> bool,
    ) -> Vec<usize> {
        let q = self.query.to_lowercase();
        let mut visible: Vec<usize> = photos
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                self.folder_scope
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&p.folder))
                    && (self.collection.is_none() || self.members.contains(&p.id))
                    && self
                        .label_filter
                        .as_ref()
                        .is_none_or(|label| &p.label == label)
                    && p.rating >= self.rating
                    && (self.flag == 2 || p.flag == self.flag)
                    && (!self.only_missing || !available(&p.path))
                    && (q.is_empty()
                        || format!(
                            "{} {} {} {} {}",
                            p.filename, p.copy_name, p.keywords, p.captured, p.label
                        )
                        .to_lowercase()
                        .contains(&q))
            })
            .map(|(i, _)| i)
            .collect();
        if self.reverse {
            visible.reverse()
        }
        visible
    }
    /// Whether the filter bar hides anything.
    pub(super) fn bar_active(&self) -> bool {
        !self.query.is_empty() || self.rating > 0 || self.flag != 2 || self.label_filter.is_some()
    }
    /// Filters Off: clears the filter bar, leaving the source as it is.
    pub(super) fn clear_bar(&mut self) {
        self.query.clear();
        self.rating = 0;
        self.flag = 2;
        self.label_filter = None;
    }
}
