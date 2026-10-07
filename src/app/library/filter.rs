//! What the Library shows: the source (a folder scope or collection), the
//! filter bar, the offline filter and the sort order. The bar follows
//! Lightroom's Attribute filter: any combination of flags, a rating compared
//! with ≥, ≤ or =, any set of color labels, and masters or virtual copies.
//! Cmd+L turns the bar off and on without losing what is set in it.
use crate::app::photo_metadata::LABELS;
use crate::catalog::Photo;
use std::{
    collections::{BTreeSet, HashSet},
    path::Path,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum RatingOp {
    #[default]
    AtLeast,
    AtMost,
    Exactly,
}
impl RatingOp {
    pub(super) const ALL: [Self; 3] = [Self::AtLeast, Self::AtMost, Self::Exactly];
    pub(super) fn symbol(self) -> &'static str {
        match self {
            Self::AtLeast => "≥",
            Self::AtMost => "≤",
            Self::Exactly => "=",
        }
    }
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::AtLeast => "Rating is greater than or equal to",
            Self::AtMost => "Rating is less than or equal to",
            Self::Exactly => "Rating is equal to",
        }
    }
    fn holds(self, rating: i32, limit: i32) -> bool {
        match self {
            Self::AtLeast => rating >= limit,
            Self::AtMost => rating <= limit,
            Self::Exactly => rating == limit,
        }
    }
}

/// A color label the bar can match.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Label {
    Color(String),
    /// No label at all.
    None,
    /// Any label other than the five colors, such as a custom label set's.
    Other,
}
impl Label {
    fn matches(&self, label: &str) -> bool {
        match self {
            Self::Color(name) => label == name,
            Self::None => label.is_empty(),
            Self::Other => !label.is_empty() && !LABELS.contains(&label),
        }
    }
}

/// Masters, virtual copies or both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Kind {
    #[default]
    All,
    Masters,
    Copies,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Filters {
    /// Folder ids in the chosen folder and its subfolders; None is All Photographs.
    pub folder_scope: Option<HashSet<crate::catalog::FolderId>>,
    pub collection: Option<crate::catalog::CollectionId>,
    /// The chosen collection's photos.
    pub members: HashSet<i64>,
    pub query: String,
    /// The flags shown (1 picked, 0 unflagged, -1 rejected); none is every flag.
    pub flags: BTreeSet<i32>,
    /// The rating compared with `rating_op`; None is every rating.
    pub rating: Option<i32>,
    pub rating_op: RatingOp,
    /// The labels shown; none is every label.
    pub labels: BTreeSet<Label>,
    pub kind: Kind,
    /// Cmd+L: whether the bar's settings apply.
    pub enabled: bool,
    pub only_missing: bool,
    pub sort: super::sort::Sort,
    pub reverse: bool,
}
impl Default for Filters {
    fn default() -> Self {
        Self {
            folder_scope: None,
            collection: None,
            members: HashSet::new(),
            query: String::new(),
            flags: BTreeSet::new(),
            rating: None,
            rating_op: RatingOp::default(),
            labels: BTreeSet::new(),
            kind: Kind::default(),
            enabled: true,
            only_missing: false,
            sort: Default::default(),
            reverse: false,
        }
    }
}
impl Filters {
    /// Indices into `photos` of the ones shown, in display order; `keys`
    /// are what the sort order needs (see `Sort::keys`).
    pub(super) fn visible(
        &self,
        photos: &[Photo],
        available: impl Fn(&Path) -> bool,
        keys: &super::sort::Keys,
    ) -> Vec<usize> {
        let query = self.query.to_lowercase();
        let mut visible: Vec<usize> = photos
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                self.folder_scope
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&p.folder))
                    && (self.collection.is_none() || self.members.contains(&p.id))
                    && (!self.only_missing || !available(&p.path))
                    && (!self.enabled || self.bar_shows(p, &query))
            })
            .map(|(i, _)| i)
            .collect();
        self.sort.sort(photos, &mut visible, keys, self.reverse);
        visible
    }
    /// Whether the filter bar lets `p` through; `query` is in lower case.
    fn bar_shows(&self, p: &Photo, query: &str) -> bool {
        (self.flags.is_empty() || self.flags.contains(&p.flag))
            && self
                .rating
                .is_none_or(|limit| self.rating_op.holds(p.rating, limit))
            && (self.labels.is_empty() || self.labels.iter().any(|l| l.matches(&p.label)))
            && match self.kind {
                Kind::All => true,
                Kind::Masters => p.master.is_none(),
                Kind::Copies => p.master.is_some(),
            }
            && (query.is_empty()
                || format!(
                    "{} {} {} {} {}",
                    p.filename, p.copy_name, p.keywords, p.captured, p.label
                )
                .to_lowercase()
                .contains(query))
    }
    /// Whether anything is set in the bar, applied or not.
    pub(super) fn bar_set(&self) -> bool {
        !self.query.is_empty()
            || !self.flags.is_empty()
            || self.rating.is_some()
            || !self.labels.is_empty()
            || self.kind != Kind::All
    }
    /// Filters Off: clears the filter bar, leaving the source as it is.
    pub(super) fn clear_bar(&mut self) {
        self.query.clear();
        self.flags.clear();
        self.rating = None;
        self.rating_op = RatingOp::default();
        self.labels.clear();
        self.kind = Kind::All;
        self.enabled = true;
    }
}

/// Adds `item` to `set`, or takes it out if it is there.
pub(super) fn toggle<T: Ord>(set: &mut BTreeSet<T>, item: T) {
    if !set.remove(&item) {
        set.insert(item);
    }
}
