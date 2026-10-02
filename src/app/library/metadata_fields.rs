//! Title, Caption, Creator, Copyright and Location in the Metadata panel and
//! the keywords in Keywording, edited in place like the Copy Name: saved on
//! Return or when focus leaves, for every photo the panel shows. A field whose
//! photos differ shows "< mixed >"; typing replaces it on all of them, leaving
//! it alone changes nothing.
use super::Library;
use super::descriptive::{DescriptiveEdit, parse_keywords};
use crate::app::theme;
use crate::catalog::{Descriptive, Keyword, Location, TextField, Value};
use eframe::egui::{self, Vec2};

const MIXED: &str = "< mixed >";
const ROW: f32 = 20.;
const CAPTION: f32 = 46.;

/// A field's value across the photos shown.
#[derive(Clone, Debug, Default, PartialEq)]
enum Shared<T> {
    #[default]
    None,
    Same(T),
    Mixed,
}
impl<T: PartialEq + Clone> Shared<T> {
    fn of(values: impl IntoIterator<Item = T>) -> Self {
        let mut shared = Self::None;
        for v in values {
            shared = match shared {
                Self::None => Self::Same(v),
                Self::Same(s) if s == v => Self::Same(s),
                _ => Self::Mixed,
            };
        }
        shared
    }
}

/// What the panel shows, and what is being typed, for `targets`.
#[derive(Default)]
pub(super) struct Fields {
    /// The photos shown; edits go to all of them.
    pub(super) targets: Vec<i64>,
    /// Read again before the next frame draws.
    stale: bool,
    title: Shared<String>,
    caption: Shared<String>,
    copyright: Shared<String>,
    creators: Shared<Vec<String>>,
    location: Shared<Option<Location>>,
    /// Each keyword, and whether every photo shown has it.
    keywords: Vec<(Keyword, bool)>,
    /// Text as typed, from the value shown when typing started.
    pub(super) drafts: Drafts,
    keyword_entry: String,
}
#[derive(Clone, Default, PartialEq)]
pub(super) struct Drafts {
    pub(super) title: String,
    caption: String,
    copyright: String,
    creators: Vec<String>,
}
impl Fields {
    /// Reads the shown values again before the next frame.
    pub(super) fn reload(&mut self) {
        self.stale = true;
    }
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
    /// The drafts as they are when nothing was typed.
    fn untouched(&self) -> Drafts {
        let text = |s: &Shared<String>| match s {
            Shared::Same(t) => t.clone(),
            _ => String::new(),
        };
        Drafts {
            title: text(&self.title),
            caption: text(&self.caption),
            copyright: text(&self.copyright),
            creators: match &self.creators {
                Shared::Same(names) if !names.is_empty() => names.clone(),
                _ => vec![String::new()],
            },
        }
    }
}

/// The default language's text of a field, as the panel shows it.
fn text_of(d: &Descriptive, field: TextField) -> String {
    match d.text(field) {
        Some(Value::Set(langs)) => langs.default_text().unwrap_or_default().to_string(),
        _ => String::new(),
    }
}

impl Library {
    /// The photos the Metadata panel edits: every one selected in the Grid,
    /// else the active one.
    fn field_targets(&self) -> Vec<i64> {
        if self.edits_active_only() {
            self.selection.active.into_iter().collect()
        } else {
            self.selected_ids()
        }
    }
    /// Saves what is being typed, for the photos it was typed for. The
    /// drafts are kept on failure.
    pub(super) fn commit_fields(&mut self) -> anyhow::Result<()> {
        let targets = self.fields.targets.clone();
        let untouched = self.fields.untouched();
        let drafts = self.fields.drafts.clone();
        let mut edits = Vec::new();
        for (field, now, was) in [
            (TextField::Title, &drafts.title, &untouched.title),
            (TextField::Caption, &drafts.caption, &untouched.caption),
            (
                TextField::Copyright,
                &drafts.copyright,
                &untouched.copyright,
            ),
        ] {
            if now != was {
                edits.push(DescriptiveEdit::Text(field, now.trim_end().to_string()));
            }
        }
        let names = |list: &[String]| -> Vec<String> {
            list.iter()
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty())
                .collect()
        };
        // An entry added and left empty changes nothing.
        let creators = names(&drafts.creators);
        if creators != names(&untouched.creators) {
            edits.push(DescriptiveEdit::Creators(creators));
        }
        for edit in edits {
            self.edit_descriptive(&targets, edit)?;
        }
        self.fields.reload();
        Ok(())
    }
    /// Reads the values shown when the photos shown changed or were edited,
    /// first saving what was typed for the ones before.
    pub(super) fn sync_fields(&mut self) {
        let targets = self.field_targets();
        if targets == self.fields.targets && !self.fields.stale {
            return;
        }
        if targets != self.fields.targets
            && let Err(e) = self.commit_fields()
        {
            self.message = format!("Metadata could not be saved: {e}");
        }
        let read = self.catalog.descriptive_of(&targets);
        let keywords: anyhow::Result<Vec<Vec<Keyword>>> = targets
            .iter()
            .map(|id| self.catalog.keywords(*id))
            .collect();
        let (read, keywords) = match (read, keywords) {
            (Ok(r), Ok(k)) => (r, k),
            (Err(e), _) | (_, Err(e)) => {
                self.message = format!("Metadata could not be read: {e}");
                return;
            }
        };
        let all: Vec<&Descriptive> = targets.iter().filter_map(|id| read.get(id)).collect();
        let f = &mut self.fields;
        f.targets = targets;
        f.stale = false;
        f.title = Shared::of(all.iter().map(|d| text_of(d, TextField::Title)));
        f.caption = Shared::of(all.iter().map(|d| text_of(d, TextField::Caption)));
        f.copyright = Shared::of(all.iter().map(|d| text_of(d, TextField::Copyright)));
        f.creators = Shared::of(all.iter().map(|d| match &d.creator {
            Some(Value::Set(names)) => names.clone(),
            _ => Vec::new(),
        }));
        f.location = Shared::of(all.iter().map(|d| d.location.clone()));
        let mut counts: Vec<(Keyword, usize)> = Vec::new();
        for k in keywords.iter().flatten() {
            match counts.iter_mut().find(|(c, _)| c.id == k.id) {
                Some((_, n)) => *n += 1,
                None => counts.push((k.clone(), 1)),
            }
        }
        counts.sort_by(|a, b| a.0.path.cmp(&b.0.path));
        let n = keywords.len();
        f.keywords = counts.into_iter().map(|(k, c)| (k, c == n)).collect();
        f.drafts = f.untouched();
    }
    /// Title, Caption, Creator, Copyright and Location rows.
    pub(super) fn metadata_fields(&mut self, ui: &mut egui::Ui) {
        self.sync_fields();
        let mut commit = false;
        let enabled = !self.fields.targets.is_empty();
        ui.add_enabled_ui(enabled, |ui| {
            let f = &mut self.fields;
            commit |= text_row(ui, "Title", &mut f.drafts.title, &f.title, false);
            commit |= text_row(ui, "Caption", &mut f.drafts.caption, &f.caption, true);
            commit |= creator_rows(ui, &mut f.drafts.creators, &f.creators);
            commit |= text_row(
                ui,
                "Copyright",
                &mut f.drafts.copyright,
                &f.copyright,
                false,
            );
        });
        if commit && let Err(e) = self.commit_fields() {
            self.message = format!("Metadata could not be saved: {e}");
        }
        let (text, can_clear) = match &self.fields.location {
            Shared::None | Shared::Same(None) => (String::new(), enabled),
            Shared::Same(Some(Location::At { lat, lon, .. })) => {
                (format!("{lat:.6}, {lon:.6}"), true)
            }
            Shared::Same(Some(Location::Cleared)) => ("Removed".into(), false),
            Shared::Mixed => (MIXED.into(), true),
        };
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), egui::Sense::hover());
        caption_at(ui, rect, "GPS");
        let button = egui::Rect::from_min_max(
            egui::pos2(rect.right() - 40., rect.top()),
            rect.right_bottom(),
        );
        value_at(
            ui,
            egui::Rect::from_min_max(rect.min, egui::pos2(button.left() - 4., rect.bottom())),
            if text.is_empty() { "—" } else { &text },
            !text.is_empty(),
        );
        response.on_hover_text(if text.is_empty() {
            "The location in the file, if it has one"
        } else {
            &text
        });
        let clear = ui
            .put(
                button,
                egui::Button::new(egui::RichText::new("Clear").size(11.)).frame(false),
            )
            .on_hover_text("Clear Location: leave it out of exports, the file's included");
        if clear.clicked() && can_clear {
            let targets = self.fields.targets.clone();
            if let Err(e) = self.edit_descriptive(&targets, DescriptiveEdit::ClearLocation) {
                self.message = format!("Location could not be cleared: {e}");
            }
        }
    }
    /// The Keywording panel: the keywords of the photos shown, one per row
    /// (with "*" when only some have it), and a field to add more.
    pub(super) fn keyword_fields(&mut self, ui: &mut egui::Ui) {
        self.sync_fields();
        let mut remove = None;
        for (keyword, all) in &self.fields.keywords {
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), egui::Sense::hover());
            let name = format!(
                "{}{}",
                keyword.path.join(" > "),
                if *all { "" } else { " *" }
            );
            let button = egui::Rect::from_min_max(
                egui::pos2(rect.right() - 20., rect.top()),
                rect.right_bottom(),
            );
            let galley = egui::WidgetText::from(name.as_str()).into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                (button.left() - rect.left()).max(1.),
                egui::FontId::proportional(11.),
            );
            ui.painter().galley(
                egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.),
                galley,
                theme::gray(205),
            );
            let tip = if *all {
                "Remove this keyword".to_string()
            } else {
                "Some of the selected photos have it (*); remove it from them".to_string()
            };
            if ui
                .put(
                    button,
                    egui::Button::new(egui::RichText::new("×").size(12.)).frame(false),
                )
                .on_hover_text(tip)
                .clicked()
            {
                remove = Some(keyword.id);
            }
        }
        let targets = self.fields.targets.clone();
        if let Some(keyword) = remove
            && let Err(e) = self.edit_descriptive(&targets, DescriptiveEdit::RemoveKeyword(keyword))
        {
            self.message = format!("Keyword could not be removed: {e}");
        }
        ui.add_space(4.);
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), egui::Sense::hover());
        let response = ui.add_enabled(!targets.is_empty(), |ui: &mut egui::Ui| {
            ui.put(
                rect.shrink2(Vec2::new(0., 1.)),
                egui::TextEdit::singleline(&mut self.fields.keyword_entry)
                    .hint_text("Add keywords: Child < Parent, …")
                    .font(egui::FontId::proportional(11.))
                    .text_color(theme::gray(205))
                    .margin(egui::Margin::symmetric(4, 1))
                    .vertical_align(egui::Align::Center),
            )
        });
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let added = parse_keywords(&self.fields.keyword_entry).and_then(|paths| {
                self.edit_descriptive(&targets, DescriptiveEdit::AddKeywords(paths))
            });
            match added {
                Ok(()) => self.fields.keyword_entry.clear(),
                Err(e) => self.message = format!("Keywords not added: {e}"),
            }
            response.request_focus();
        }
    }
}

/// The caption column of a row.
fn caption_at(ui: &egui::Ui, rect: egui::Rect, key: &str) {
    ui.painter().text(
        egui::pos2(rect.left() + 84., rect.top() + ROW / 2.),
        egui::Align2::RIGHT_CENTER,
        key,
        egui::FontId::proportional(11.),
        theme::gray(135),
    );
}
fn value_at(ui: &egui::Ui, rect: egui::Rect, text: &str, set: bool) {
    let left = rect.left() + 92.;
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (rect.right() - left).max(1.),
        egui::FontId::proportional(11.),
    );
    ui.painter().galley(
        egui::pos2(left, rect.center().y - galley.size().y / 2.),
        galley,
        theme::gray(if set { 205 } else { 90 }),
    );
}
/// The text field area of a row, right of its caption.
fn field_rect(rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(rect.left() + 88., rect.top() + 1.),
        egui::pos2(rect.right(), rect.bottom() - 1.),
    )
}
fn edit<'t>(text: &'t mut String, shared: &Shared<String>, multiline: bool) -> egui::TextEdit<'t> {
    let edit = if multiline {
        egui::TextEdit::multiline(text).desired_rows(3)
    } else {
        egui::TextEdit::singleline(text)
    };
    edit.hint_text(if *shared == Shared::Mixed { MIXED } else { "" })
        .font(egui::FontId::proportional(11.))
        .text_color(theme::gray(205))
        .margin(egui::Margin::symmetric(4, 1))
}
/// A text row; true when it was left, to save it. The caption is a fixed
/// three lines that scroll, so the panel never reflows.
fn text_row(
    ui: &mut egui::Ui,
    key: &str,
    text: &mut String,
    shared: &Shared<String>,
    multiline: bool,
) -> bool {
    let height = if multiline { CAPTION } else { ROW };
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height),
        egui::Sense::hover(),
    );
    caption_at(ui, rect, key);
    let field = field_rect(rect);
    let response = if multiline {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field));
        egui::ScrollArea::vertical()
            .id_salt(("metadata-field", key))
            .max_height(field.height())
            .show(&mut child, |ui| {
                ui.add_sized(
                    Vec2::new(field.width(), field.height()),
                    edit(text, shared, true),
                )
            })
            .inner
    } else {
        ui.put(
            field,
            edit(text, shared, false).vertical_align(egui::Align::Center),
        )
    };
    response.lost_focus()
}
/// One row per creator, each removable, and a button to add one; true when
/// the list changed or an entry was left, to save it.
fn creator_rows(ui: &mut egui::Ui, names: &mut Vec<String>, shared: &Shared<Vec<String>>) -> bool {
    let mut commit = false;
    let mut remove = None;
    let count = names.len();
    for (i, name) in names.iter_mut().enumerate() {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), egui::Sense::hover());
        if i == 0 {
            caption_at(ui, rect, "Creator");
        }
        let field = field_rect(rect);
        let button = egui::Rect::from_min_max(
            egui::pos2(field.right() - 18., field.top()),
            field.right_bottom(),
        );
        let text =
            egui::Rect::from_min_max(field.min, egui::pos2(button.left() - 2., field.bottom()));
        let hint = if *shared == Shared::Mixed && count == 1 {
            MIXED
        } else {
            ""
        };
        let response = ui.put(
            text,
            egui::TextEdit::singleline(name)
                .hint_text(hint)
                .font(egui::FontId::proportional(11.))
                .text_color(theme::gray(205))
                .margin(egui::Margin::symmetric(4, 1))
                .vertical_align(egui::Align::Center),
        );
        commit |= response.lost_focus();
        let last = i + 1 == count;
        let (label, tip) = if last {
            ("+", "Add a creator")
        } else {
            ("−", "Remove this creator")
        };
        if ui
            .put(
                button,
                egui::Button::new(egui::RichText::new(label).size(12.)).frame(false),
            )
            .on_hover_text(tip)
            .clicked()
        {
            if last {
                remove = Some(usize::MAX);
            } else {
                remove = Some(i);
            }
        }
    }
    match remove {
        // "+" adds an empty entry to type in, saved when it is left.
        Some(usize::MAX) => names.push(String::new()),
        Some(i) => {
            names.remove(i);
            commit = true;
        }
        None => {}
    }
    commit
}
