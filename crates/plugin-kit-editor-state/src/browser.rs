//! Preset list and drawer state, from the CA-72's R10, R18 and R44 and the CA-74's
//! `presets::Browser`. The caller copies metadata from its library's entries into
//! [`Preset`], translates hit targets and keys, and executes returned requests.
//! No file, parameter, window, layout, animation or MIDI implementation is held here.
//!
//! Prompts follow a name and origin, never a row index. As in the newer CA-72,
//! filtering a preset out keeps its prompt; removing it, changing its origin or
//! choosing another ends it. Before executing a request, the caller re-reads its
//! library and checks that the named preset still has the request's origin. A
//! completion token prevents an old reply from ending a newer prompt.

use crate::text::{TextEdit, TextField};

/// The library's three provenances, copied without its parameter data or file handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Factory,
    Edited,
    User,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub favorite: bool,
    pub origin: Origin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresetId {
    pub name: String,
    pub origin: Origin,
}

impl Preset {
    pub fn id(&self) -> PresetId {
        PresetId {
            name: self.name.clone(),
            origin: self.origin,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    All,
    Favourites,
    Mine,
    /// Optional, as in the later CA-72 and CA-74: only unmodified factory entries.
    Factory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldId {
    Search,
    Edit,
    SaveName,
    SaveTags,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prompt {
    Rename { preset: PresetId, field: TextField },
    Tags { preset: PresetId, field: TextField },
    Delete { preset: PresetId },
    SaveAs { name: TextField, tags: TextField },
}

impl Prompt {
    pub fn preset(&self) -> Option<&PresetId> {
        match self {
            Self::Rename { preset, .. } | Self::Tags { preset, .. } | Self::Delete { preset } => {
                Some(preset)
            }
            Self::SaveAs { .. } => None,
        }
    }
}

/// Semantic keys and caller hit targets. `close` is supplied from the caller's
/// double-click detection. INIT and preset pads are optional caller actions, not rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserAction {
    Rename,
    Tags,
    Delete,
    Revert,
    SaveAs,
    Restore,
    MidiLearn,
    Close,
    Choose { row: usize, close: bool },
    Favorite(usize),
    Init,
    PresetPad(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserKey<'a> {
    Text(TextEdit<'a>),
    Enter,
    Escape,
    Tab,
    Previous,
    Next,
}

/// Work belonging to the caller. Tags are the whole field, for the library's parser;
/// names and tags must be validated there before writing. SAVE AS captures the
/// caller's current sound, and INIT restores its own defaults and player exclusions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    Rename { preset: PresetId, to: String },
    Tags { preset: PresetId, text: String },
    Delete(PresetId),
    Revert(PresetId),
    SaveAs { name: String, tags: String },
    Favorite { preset: PresetId, on: bool },
    Restore,
    MidiLearn,
    Init,
    PresetPad(usize),
}

impl Operation {
    fn preset(&self) -> Option<&PresetId> {
        match self {
            Self::Rename { preset, .. }
            | Self::Tags { preset, .. }
            | Self::Delete(preset)
            | Self::Revert(preset)
            | Self::Favorite { preset, .. } => Some(preset),
            _ => None,
        }
    }
}

/// Echo this token to `complete` after executing the operation. Only one request
/// is pending at once. Cancelling its UI does not cancel work the caller already began.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub token: u64,
    pub operation: Operation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    NoPreset,
    NotEdited,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserEffect {
    Choose(PresetId),
    Request(Request),
    Unavailable(Unavailable),
    Changed,
    Close,
}

/// A successful operation may explicitly report the caller's new current preset,
/// for example after rename, save, revert or INIT. `Changed` leaves it alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Completion {
    Changed,
    Current(Option<String>),
}

#[derive(Clone, Debug)]
struct Pending {
    request: Request,
    ends_prompt: bool,
}

/// A measured drawing adapter's view fields, independent of any colours or units.
#[derive(Clone, Debug)]
pub struct BrowserView {
    pub search: TextField,
    pub tab: Tab,
    pub picked: Vec<String>,
    pub first: usize,
    pub tags_first: usize,
    pub tag_columns: usize,
    pub tag_rows: usize,
    pub focus: Option<FieldId>,
}

#[derive(Clone, Debug, Default)]
pub struct BrowserState {
    open: bool,
    prompts_require_visible_row: bool,
    all: Vec<Preset>,
    list: Vec<Preset>,
    search: TextField,
    tab: Tab,
    picked: Vec<String>,
    chips: Vec<(String, bool)>,
    current: Option<String>,
    first: usize,
    visible: usize,
    tags_first: usize,
    tag_columns: usize,
    tag_rows: usize,
    prompt: Option<Prompt>,
    focus: Option<FieldId>,
    pending: Option<Pending>,
    next_token: u64,
    error: Option<String>,
    told: Option<Vec<String>>,
}

fn same(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}
fn matches_id(preset: &Preset, id: &PresetId) -> bool {
    same(&preset.name, &id.name) && preset.origin == id.origin
}
fn clamp_first(first: usize, count: usize, visible: usize) -> usize {
    if visible == 0 {
        0
    } else {
        first.min(count.saturating_sub(visible))
    }
}

impl BrowserState {
    pub fn new(visible_rows: usize) -> Self {
        Self {
            visible: visible_rows,
            tag_columns: 1,
            ..Self::default()
        }
    }

    /// CA-74 R37/R-PRESET-KEYS ends a prompt when filtering removes its row.
    /// The CA-72 default keeps a prompt while its preset still exists.
    pub fn set_prompts_require_visible_row(&mut self, on: bool) {
        self.prompts_require_visible_row = on;
        self.refilter();
    }

    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn all(&self) -> &[Preset] {
        &self.all
    }
    pub fn list(&self) -> &[Preset] {
        &self.list
    }
    pub fn tab(&self) -> Tab {
        self.tab
    }
    pub fn chips(&self) -> &[(String, bool)] {
        &self.chips
    }
    pub fn first(&self) -> usize {
        self.first
    }
    pub fn tags_first(&self) -> usize {
        self.tags_first
    }
    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref()
    }
    pub fn focus(&self) -> Option<FieldId> {
        self.focus
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn pending(&self) -> Option<&Request> {
        self.pending.as_ref().map(|p| &p.request)
    }
    pub fn current_name(&self) -> Option<&str> {
        self.current.as_deref()
    }
    pub fn current(&self) -> Option<&Preset> {
        let name = self.current.as_deref()?;
        self.all.iter().find(|e| same(&e.name, name))
    }
    pub fn selected_row(&self) -> Option<usize> {
        let name = self.current.as_deref()?;
        self.list.iter().position(|e| same(&e.name, name))
    }

    pub fn field(&self, id: FieldId) -> Option<&TextField> {
        match (id, self.prompt.as_ref()) {
            (FieldId::Search, _) => Some(&self.search),
            (FieldId::Edit, Some(Prompt::Rename { field, .. } | Prompt::Tags { field, .. })) => {
                Some(field)
            }
            (FieldId::SaveName, Some(Prompt::SaveAs { name, .. })) => Some(name),
            (FieldId::SaveTags, Some(Prompt::SaveAs { tags, .. })) => Some(tags),
            _ => None,
        }
    }

    pub fn field_mut(&mut self, id: FieldId) -> Option<&mut TextField> {
        match (id, self.prompt.as_mut()) {
            (FieldId::Search, _) => Some(&mut self.search),
            (FieldId::Edit, Some(Prompt::Rename { field, .. } | Prompt::Tags { field, .. })) => {
                Some(field)
            }
            (FieldId::SaveName, Some(Prompt::SaveAs { name, .. })) => Some(name),
            (FieldId::SaveTags, Some(Prompt::SaveAs { tags, .. })) => Some(tags),
            _ => None,
        }
    }

    /// The adapter maps a measured field hit to its character caret.
    pub fn focus_field(&mut self, id: FieldId, caret: usize) -> bool {
        if !self.open {
            return false;
        }
        let Some(field) = self.field_mut(id) else {
            return false;
        };
        field.set_caret(caret);
        self.focus = Some(id);
        true
    }

    pub fn end_prompt(&mut self) {
        self.prompt = None;
        self.pending = None;
        self.focus = self.open.then_some(FieldId::Search);
    }

    pub fn show(&mut self, on: bool) {
        if self.open == on && on {
            return;
        }
        self.open = on;
        self.end_prompt();
        self.error = None;
    }

    /// A host or caller changed the current preset. Its name survives a refresh
    /// which removes it, so the adapter can still show the last sound's name.
    pub fn set_current(&mut self, name: Option<&str>) {
        let unchanged = match (self.current.as_deref(), name) {
            (Some(a), Some(b)) => same(a, b),
            (None, None) => true,
            _ => false,
        };
        if !unchanged {
            self.end_prompt();
            self.error = None;
        }
        self.current = name.map(str::to_owned);
    }

    /// Import a measured view without treating an unchanged filter as a new search.
    /// This lets a drawing adapter retain editable view fields without another browser model.
    pub fn set_view(&mut self, view: BrowserView) {
        self.search = view.search;
        self.tab = view.tab;
        self.picked = view.picked;
        self.first = view.first;
        self.tags_first = view.tags_first;
        self.tag_columns = view.tag_columns;
        self.tag_rows = view.tag_rows;
        self.focus = view.focus;
        self.refilter();
    }

    /// Observe a host's preset change. A SAVE AS still holds its proposed name;
    /// a prompt bound to the previous sound ends (CA-72 R18).
    pub fn observe_current(&mut self, name: Option<&str>) {
        if self
            .prompt
            .as_ref()
            .and_then(Prompt::preset)
            .is_some_and(|p| name.is_none_or(|name| !same(&p.name, name)))
        {
            self.end_prompt();
        }
        self.current = name.map(str::to_owned);
    }

    /// Replace the metadata snapshot copied from `plugin-kit-library::Entry`.
    /// No file is read for filtering. Selection follows the name, prompts also
    /// require the same origin, and a visible selection stays in view after sorting.
    pub fn refresh(&mut self, presets: impl IntoIterator<Item = Preset>) {
        self.all = presets.into_iter().collect();
        self.all.sort_by_key(|e| e.name.to_lowercase());
        self.refilter();
        let prompt_vanished = self
            .prompt
            .as_ref()
            .and_then(Prompt::preset)
            .is_some_and(|id| !self.all.iter().any(|e| matches_id(e, id)));
        let request_vanished = self
            .pending
            .as_ref()
            .and_then(|p| p.request.operation.preset())
            .is_some_and(|id| !self.all.iter().any(|e| matches_id(e, id)));
        if prompt_vanished {
            self.end_prompt();
        } else if request_vanished {
            self.pending = None;
        }
        self.first = clamp_first(self.first, self.list.len(), self.visible);
    }

    fn refilter(&mut self) {
        let words: Vec<_> = self
            .search
            .text()
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        self.list = self
            .all
            .iter()
            .filter(|e| {
                let text =
                    format!("{} {} {}", e.name, e.description, e.tags.join(" ")).to_lowercase();
                words.iter().all(|word| text.contains(word))
                    && self
                        .picked
                        .iter()
                        .all(|tag| e.tags.iter().any(|t| same(tag, t)))
                    && match self.tab {
                        Tab::All => true,
                        Tab::Favourites => e.favorite,
                        Tab::Mine => e.origin != Origin::Factory,
                        Tab::Factory => e.origin == Origin::Factory,
                    }
            })
            .cloned()
            .collect();
        let mut tags: Vec<String> = Vec::new();
        for tag in self.all.iter().flat_map(|e| &e.tags) {
            if !tags.iter().any(|t| same(t, tag)) {
                tags.push(tag.clone());
            }
        }
        tags.sort_by_key(|t| t.to_lowercase());
        for tag in &self.picked {
            if !tags.iter().any(|t| same(t, tag)) {
                tags.push(tag.clone());
            }
        }
        self.chips = tags
            .into_iter()
            .map(|tag| {
                let on = self.picked.iter().any(|t| same(t, &tag));
                (tag, on)
            })
            .collect();
        if self.prompts_require_visible_row
            && self
                .prompt
                .as_ref()
                .and_then(Prompt::preset)
                .is_some_and(|id| !self.list.iter().any(|p| matches_id(p, id)))
        {
            self.end_prompt();
        }
        self.first = clamp_first(self.first, self.list.len(), self.visible);
        self.tags_first = clamp_first(
            self.tags_first,
            self.chips.len().div_ceil(self.tag_columns.max(1)),
            self.tag_rows,
        );
    }

    pub fn set_search(&mut self, text: impl Into<String>) {
        self.search = TextField::new(text);
        self.first = 0;
        self.refilter();
    }

    pub fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.first = 0;
        self.refilter();
    }

    pub fn toggle_tag(&mut self, tag: &str) {
        if let Some(i) = self.picked.iter().position(|t| same(t, tag)) {
            self.picked.remove(i);
        } else {
            self.picked.push(tag.to_owned());
        }
        self.first = 0;
        self.refilter();
    }

    pub fn set_visible_rows(&mut self, visible: usize) {
        self.visible = visible;
        self.first = clamp_first(self.first, self.list.len(), visible);
    }

    pub fn scroll(&mut self, rows: isize, visible: usize) {
        self.set_visible_rows(visible);
        self.first = clamp_first(
            self.first.saturating_add_signed(rows),
            self.list.len(),
            visible,
        );
    }

    /// Tag layout is the caller's too; columns and visible rows are supplied after
    /// measuring. Zero columns or visible rows show no rows and reset the scroll.
    pub fn scroll_tags(&mut self, rows: isize, columns: usize, visible: usize) {
        self.tag_columns = columns;
        self.tag_rows = if columns == 0 { 0 } else { visible };
        self.tags_first = clamp_first(
            self.tags_first.saturating_add_signed(rows),
            self.chips.len().div_ceil(columns.max(1)),
            self.tag_rows,
        );
    }

    pub fn bring_current_into_view(&mut self) {
        self.bring_into_view();
    }

    fn bring_into_view(&mut self) {
        let Some(i) = self.selected_row() else {
            return;
        };
        if self.visible == 0 {
            self.first = 0;
        } else if i < self.first {
            self.first = i;
        } else if i - self.first >= self.visible {
            self.first = i + 1 - self.visible;
        }
    }

    /// Previous and next wrap within the filtered set. An absent current preset
    /// starts at the first (forward) or last (backward); an empty set changes nothing.
    pub fn step(&mut self, by: isize) -> Vec<BrowserEffect> {
        if self.list.is_empty() {
            return Vec::new();
        }
        let n = self.list.len();
        let i = match self.selected_row() {
            Some(i) if by >= 0 => (i + by as usize % n) % n,
            Some(i) => (i + n - by.unsigned_abs() % n) % n,
            None if by > 0 => 0,
            None => n - 1,
        };
        self.choose(i, false)
    }

    fn choose(&mut self, row: usize, close: bool) -> Vec<BrowserEffect> {
        let Some(id) = self.list.get(row).map(Preset::id) else {
            return Vec::new();
        };
        self.end_prompt();
        self.error = None;
        self.current = Some(id.name.clone());
        self.bring_into_view();
        let mut effects = vec![BrowserEffect::Changed, BrowserEffect::Choose(id)];
        if close {
            self.show(false);
            effects.push(BrowserEffect::Close);
        }
        effects
    }

    /// The names that preset pads step through, only when that set or order changes.
    pub fn list_changed(&mut self) -> Option<Vec<String>> {
        let names: Vec<_> = self.list.iter().map(|e| e.name.clone()).collect();
        if self.told.as_ref() == Some(&names) {
            return None;
        }
        self.told = Some(names.clone());
        Some(names)
    }

    /// The existing preset a save name would replace, for the caller's warning.
    pub fn replacement(&self) -> Option<&Preset> {
        let Prompt::SaveAs { name, .. } = self.prompt.as_ref()? else {
            return None;
        };
        self.all.iter().find(|e| same(&e.name, name.text().trim()))
    }

    fn request(&mut self, operation: Operation, ends_prompt: bool) -> Vec<BrowserEffect> {
        if self.pending.is_some() {
            return Vec::new();
        }
        self.next_token = self.next_token.wrapping_add(1);
        let request = Request {
            token: self.next_token,
            operation,
        };
        self.pending = Some(Pending {
            request: request.clone(),
            ends_prompt,
        });
        self.error = None;
        vec![BrowserEffect::Changed, BrowserEffect::Request(request)]
    }

    /// Begin a key's prompt, or return an operation to be performed by the caller.
    /// DELETE and SAVE AS repeat to confirm. RENAME and TAGS confirm with Enter.
    pub fn act(&mut self, action: BrowserAction) -> Vec<BrowserEffect> {
        match action {
            BrowserAction::Choose { row, close } => return self.choose(row, close),
            BrowserAction::Favorite(row) => {
                let Some(e) = self.list.get(row) else {
                    return Vec::new();
                };
                return self.request(
                    Operation::Favorite {
                        preset: e.id(),
                        on: !e.favorite,
                    },
                    false,
                );
            }
            BrowserAction::Close => {
                self.show(false);
                return vec![BrowserEffect::Changed, BrowserEffect::Close];
            }
            BrowserAction::MidiLearn | BrowserAction::Init => {
                self.end_prompt();
                let op = if action == BrowserAction::Init {
                    Operation::Init
                } else {
                    Operation::MidiLearn
                };
                return self.request(op, false);
            }
            BrowserAction::PresetPad(pad) => return self.request(Operation::PresetPad(pad), false),
            BrowserAction::Restore => {
                self.end_prompt();
                return self.request(Operation::Restore, false);
            }
            BrowserAction::SaveAs => {
                if matches!(self.prompt, Some(Prompt::SaveAs { .. })) {
                    return self.confirm();
                }
                let name = self.current().map_or(String::new(), |e| e.name.clone());
                let tags = self.current().map_or(String::new(), |e| e.tags.join(", "));
                self.show(true);
                self.end_prompt();
                self.prompt = Some(Prompt::SaveAs {
                    name: TextField::new(name),
                    tags: TextField::new(tags),
                });
                self.focus = Some(FieldId::SaveName);
            }
            action => {
                let Some(e) = self.current().cloned() else {
                    self.end_prompt();
                    return vec![
                        BrowserEffect::Changed,
                        BrowserEffect::Unavailable(Unavailable::NoPreset),
                    ];
                };
                if action == BrowserAction::Delete
                    && matches!(self.prompt, Some(Prompt::Delete { .. }))
                {
                    return self.confirm();
                }
                self.end_prompt();
                if matches!(action, BrowserAction::Rename | BrowserAction::Tags) {
                    self.focus = Some(FieldId::Edit);
                }
                self.prompt = Some(match action {
                    BrowserAction::Rename => Prompt::Rename {
                        preset: e.id(),
                        field: TextField::new(e.name),
                    },
                    BrowserAction::Tags => Prompt::Tags {
                        preset: e.id(),
                        field: TextField::new(e.tags.join(", ")),
                    },
                    BrowserAction::Delete => {
                        self.focus = self.open.then_some(FieldId::Search);
                        Prompt::Delete { preset: e.id() }
                    }
                    BrowserAction::Revert if e.origin == Origin::Edited => {
                        return self.request(Operation::Revert(e.id()), false);
                    }
                    BrowserAction::Revert => {
                        return vec![
                            BrowserEffect::Changed,
                            BrowserEffect::Unavailable(Unavailable::NotEdited),
                        ];
                    }
                    _ => unreachable!("Other actions were handled above"),
                });
            }
        }
        self.error = None;
        vec![BrowserEffect::Changed]
    }

    pub fn confirm(&mut self) -> Vec<BrowserEffect> {
        let Some(prompt) = self.prompt.as_ref() else {
            return Vec::new();
        };
        let operation = match prompt {
            Prompt::Rename { preset, field } => Operation::Rename {
                preset: preset.clone(),
                to: field.text().to_owned(),
            },
            Prompt::Tags { preset, field } => Operation::Tags {
                preset: preset.clone(),
                text: field.text().to_owned(),
            },
            Prompt::Delete { preset } => Operation::Delete(preset.clone()),
            Prompt::SaveAs { name, tags } => {
                if name.text().trim().is_empty() {
                    self.focus = Some(FieldId::SaveName);
                    return vec![BrowserEffect::Changed];
                }
                Operation::SaveAs {
                    name: name.text().trim().to_owned(),
                    tags: tags.text().to_owned(),
                }
            }
        };
        self.request(operation, true)
    }

    /// Complete only the still-current request. Failure keeps its prompt for correction;
    /// success ends a confirmed prompt. The caller then refreshes library metadata.
    pub fn complete(
        &mut self,
        token: u64,
        result: Result<Completion, String>,
    ) -> Vec<BrowserEffect> {
        if !self
            .pending
            .as_ref()
            .is_some_and(|p| p.request.token == token)
        {
            return Vec::new();
        }
        let pending = self.pending.take().expect("The token was checked");
        match result {
            Ok(completion) => {
                if pending.ends_prompt {
                    self.end_prompt();
                }
                if let Completion::Current(name) = completion {
                    self.set_current(name.as_deref());
                }
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        vec![BrowserEffect::Changed]
    }

    /// Translate only the drawer's keys here. The adapter retains host modifier
    /// shortcuts and key-up policy. Unknown keys need no browser operation.
    pub fn key(&mut self, key: BrowserKey<'_>) -> Vec<BrowserEffect> {
        if !self.open {
            return Vec::new();
        }
        match key {
            BrowserKey::Escape => {
                if self.prompt.is_some() {
                    self.end_prompt();
                    self.error = None;
                } else {
                    return self.act(BrowserAction::Close);
                }
            }
            BrowserKey::Enter => {
                if matches!(self.prompt, Some(Prompt::Delete { .. }))
                    || matches!(
                        self.focus,
                        Some(FieldId::Edit | FieldId::SaveName | FieldId::SaveTags)
                    )
                {
                    return self.confirm();
                }
                return self.act(BrowserAction::Close);
            }
            BrowserKey::Previous | BrowserKey::Next => {
                if matches!(self.focus, None | Some(FieldId::Search)) {
                    return self.step(if key == BrowserKey::Next { 1 } else { -1 });
                }
                return Vec::new();
            }
            BrowserKey::Tab => {
                self.focus = Some(match (&self.prompt, self.focus) {
                    (Some(Prompt::SaveAs { .. }), None | Some(FieldId::Search)) => {
                        FieldId::SaveName
                    }
                    (Some(Prompt::SaveAs { .. }), Some(FieldId::SaveName)) => FieldId::SaveTags,
                    (
                        Some(Prompt::Rename { .. } | Prompt::Tags { .. }),
                        None | Some(FieldId::Search),
                    ) => FieldId::Edit,
                    _ => FieldId::Search,
                });
            }
            BrowserKey::Text(edit) => {
                if self.pending.as_ref().is_some_and(|p| p.ends_prompt) {
                    return Vec::new();
                }
                let id = self.focus.unwrap_or(FieldId::Search);
                let Some(field) = self.field_mut(id) else {
                    return Vec::new();
                };
                let edited = field.edit(edit);
                self.focus = Some(id);
                if id == FieldId::Search {
                    self.first = 0;
                    self.refilter();
                }
                if edited {
                    self.error = None;
                }
            }
        }
        vec![BrowserEffect::Changed]
    }
}

#[cfg(test)]
mod tests;
