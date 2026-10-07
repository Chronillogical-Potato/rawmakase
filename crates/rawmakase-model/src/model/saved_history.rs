//! A photo's Develop History as it is kept between sessions: plain values, so the
//! History that edits it and the catalog that stores it each depend only on these.
use super::recipe::Recipe;

/// A History as saved: the state before its first step, every step with the state it
/// leaves, done steps first, and how many are applied.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedHistory {
    pub origin: Recipe,
    pub steps: Vec<SavedStep>,
    pub applied: usize,
}
#[derive(Clone, Debug, PartialEq)]
pub struct SavedStep {
    pub name: String,
    pub value: String,
    pub recipe: Recipe,
}
