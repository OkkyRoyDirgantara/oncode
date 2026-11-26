//! The model of tabs and editor groups. Pure state; its view lives in `workbench.rs`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The data of a tab. Its view is built by the `TabKind` registered for `kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabData {
    /// Unique across the whole window; a tab with the same id is not opened twice in one group.
    pub id: String,
    pub kind: String,
    pub title: String,
    /// Full text for the tooltip (e.g. the path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub params: Value,
    /// A preview tab is replaced when another preview tab is opened.
    #[serde(default)]
    pub preview: bool,
}

impl TabData {
    /// A new tab; the id is derived from its kind and parameters.
    pub fn new(kind: impl Into<String>, title: impl Into<String>, params: Value) -> Self {
        let kind = kind.into();
        let id = format!("{kind}:{params}");
        Self { id, kind, title: title.into(), description: None, params, preview: false }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// A string-typed parameter.
    pub fn param_str(&self, key: &str) -> Option<&str> {
        self.params.get(key).and_then(Value::as_str)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub id: u32,
    pub tabs: Vec<TabData>,
    pub active: Option<String>,
}

impl Group {
    pub fn active_tab(&self) -> Option<&TabData> {
        let id = self.active.as_deref()?;
        self.tabs.iter().find(|t| t.id == id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.tabs.iter().any(|t| t.id == id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    #[default]
    Horizontal,
    Vertical,
}

/// The side of a group where a new group is created when a tab is dropped on its edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitSide {
    Left,
    Right,
    Up,
    Down,
}

impl SplitSide {
    /// The layout direction of the groups that results from this side.
    pub fn orientation(self) -> Orientation {
        match self {
            SplitSide::Left | SplitSide::Right => Orientation::Horizontal,
            SplitSide::Up | SplitSide::Down => Orientation::Vertical,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    /// Open as a preview (replacing the previous preview tab).
    pub preview: bool,
    pub group: Option<u32>,
    /// Open in the adjacent group (created if it does not exist yet).
    pub to_side: bool,
    /// Do not move focus to this tab.
    pub background: bool,
}

impl OpenOptions {
    pub fn preview() -> Self {
        Self { preview: true, ..Default::default() }
    }

    pub fn to_side() -> Self {
        Self { to_side: true, ..Default::default() }
    }
}

#[derive(Debug, Clone)]
pub struct Model {
    pub groups: Vec<Group>,
    pub active_group: u32,
    pub orientation: Orientation,
    next_group: u32,
}

impl Default for Model {
    fn default() -> Self {
        Self { groups: vec![Group { id: 1, tabs: Vec::new(), active: None }], active_group: 1, orientation: Orientation::Horizontal, next_group: 2 }
    }
}

impl Model {
    fn new_group(&mut self) -> u32 {
        let id = self.next_group;
        self.next_group += 1;
        self.groups.push(Group { id, tabs: Vec::new(), active: None });
        id
    }

    pub fn group(&self, id: u32) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }

    fn group_mut(&mut self, id: u32) -> Option<&mut Group> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    pub fn active(&self) -> Option<(&Group, &TabData)> {
        let group = self.group(self.active_group)?;
        Some((group, group.active_tab()?))
    }

    pub fn all_tabs(&self) -> impl Iterator<Item = &TabData> {
        self.groups.iter().flat_map(|g| g.tabs.iter())
    }

    /// The group that holds this tab (the active group takes precedence).
    pub fn find_tab(&self, id: &str) -> Option<u32> {
        if self.group(self.active_group).is_some_and(|g| g.contains(id)) {
            return Some(self.active_group);
        }
        self.groups.iter().find(|g| g.contains(id)).map(|g| g.id)
    }

    /// Opens a tab (or activates it if it already exists). Returns the group the tab is in.
    pub fn open(&mut self, mut tab: TabData, options: &OpenOptions) -> u32 {
        tab.preview = options.preview;
        let id = tab.id.clone();
        let mut group_id = options.group.unwrap_or(self.active_group);
        if options.to_side {
            let index = self.groups.iter().position(|g| g.id == self.active_group).unwrap_or(0);
            group_id = match self.groups.get(index + 1) {
                Some(next) => next.id,
                None => self.new_group(),
            };
        } else if options.group.is_none() {
            // If the tab is already open in another group, use that one.
            let here = self.group(group_id).is_some_and(|g| g.contains(&id));
            if !here {
                if let Some(holder) = self.groups.iter().find(|g| g.contains(&id)) {
                    group_id = holder.id;
                }
            }
        }
        if self.group(group_id).is_none() {
            group_id = self.groups[0].id;
        }

        let background = options.background;
        let group = self.group_mut(group_id).expect("the group exists");
        if let Some(existing) = group.tabs.iter_mut().find(|t| t.id == id) {
            let keep_preview = existing.preview && tab.preview;
            *existing = TabData { preview: keep_preview, ..tab };
            if !background {
                group.active = Some(id);
            }
        } else {
            let preview_index = if tab.preview { group.tabs.iter().position(|t| t.preview) } else { None };
            match preview_index {
                Some(index) => group.tabs[index] = tab,
                None => {
                    let at = group.active.as_ref().and_then(|a| group.tabs.iter().position(|t| &t.id == a)).map_or(group.tabs.len(), |i| i + 1);
                    group.tabs.insert(at, tab);
                }
            }
            if !background || group.active.is_none() {
                group.active = Some(id);
            }
        }
        if !background {
            self.active_group = group_id;
        }
        group_id
    }

    pub fn activate(&mut self, group_id: u32, id: &str) {
        if let Some(group) = self.group_mut(group_id) {
            if group.contains(id) {
                group.active = Some(id.to_string());
                self.active_group = group_id;
            }
        }
    }

    pub fn activate_group(&mut self, group_id: u32) {
        if self.group(group_id).is_some() {
            self.active_group = group_id;
        }
    }

    /// Turns a preview tab into a permanent tab (e.g. when editing starts or on double click).
    pub fn pin(&mut self, id: &str) -> bool {
        let mut changed = false;
        for tab in self.groups.iter_mut().flat_map(|g| g.tabs.iter_mut()) {
            if tab.id == id && tab.preview {
                tab.preview = false;
                changed = true;
            }
        }
        changed
    }

    /// Changes tabs in all groups (e.g. after a file is renamed). The active id is updated too.
    pub fn update_tabs(&mut self, mut change: impl FnMut(&TabData) -> Option<TabData>) {
        for group in &mut self.groups {
            for tab in &mut group.tabs {
                if let Some(next) = change(tab) {
                    if group.active.as_deref() == Some(tab.id.as_str()) {
                        group.active = Some(next.id.clone());
                    }
                    *tab = next;
                }
            }
        }
    }

    /// Removes tabs from a group. An empty group is dropped, except the last group.
    pub fn remove(&mut self, group_id: u32, ids: &[String]) {
        if let Some(group) = self.group_mut(group_id) {
            if let Some(active) = group.active.clone() {
                if ids.contains(&active) {
                    let old = group.tabs.iter().position(|t| t.id == active).unwrap_or(0);
                    let after = group.tabs[old + 1..].iter().find(|t| !ids.contains(&t.id));
                    let before = group.tabs[..old].iter().rev().find(|t| !ids.contains(&t.id));
                    group.active = after.or(before).map(|t| t.id.clone());
                }
            }
            group.tabs.retain(|t| !ids.contains(&t.id));
        }
        if self.groups.len() > 1 {
            let first = self.groups[0].id;
            self.groups.retain(|g| !g.tabs.is_empty());
            if self.groups.is_empty() {
                self.groups.push(Group { id: first, tabs: Vec::new(), active: None });
            }
            if self.group(self.active_group).is_none() {
                self.active_group = self.groups[self.groups.len() - 1].id;
            }
        }
    }

    /// The next/previous tab in the active group.
    pub fn cycle(&mut self, delta: isize) {
        let active_group = self.active_group;
        let Some(group) = self.group_mut(active_group) else { return };
        if group.tabs.len() < 2 {
            return;
        }
        let len = group.tabs.len() as isize;
        let index = group.active.as_ref().and_then(|a| group.tabs.iter().position(|t| &t.id == a)).unwrap_or(0) as isize;
        let next = (index + delta).rem_euclid(len) as usize;
        group.active = Some(group.tabs[next].id.clone());
    }

    pub fn toggle_orientation(&mut self) {
        self.orientation = match self.orientation {
            Orientation::Horizontal => Orientation::Vertical,
            Orientation::Vertical => Orientation::Horizontal,
        };
    }

    /// The group that really holds this tab: `from` if it does hold it, otherwise whichever group
    /// holds it.
    fn source_of(&self, id: &str, from: Option<u32>) -> Option<u32> {
        from.filter(|g| self.group(*g).is_some_and(|g| g.contains(id))).or_else(|| self.find_tab(id))
    }

    /// Moves a tab to group `to` at position `index` (`None` = at the end), then activates it.
    /// `from` is its source group; `None` for a tab from outside the editor area (e.g. a terminal in the panel),
    /// which is treated like a new tab unless it is already open in one of the groups.
    /// Returns the other group the tab was removed from, if any.
    pub fn move_tab(&mut self, tab: TabData, from: Option<u32>, to: u32, index: Option<usize>) -> Option<u32> {
        let id = tab.id.clone();
        let from = self.source_of(&id, from);
        let target = self.group_mut(to)?;
        match target.tabs.iter().position(|t| t.id == id) {
            // Reordering within the same group (or a tab that already exists in the target group).
            Some(old) => {
                if let Some(index) = index {
                    let moved = target.tabs.remove(old);
                    // The target position is computed before the tab is removed from its old place.
                    let at = if index > old { index - 1 } else { index };
                    target.tabs.insert(at.min(target.tabs.len()), moved);
                }
            }
            None => {
                let at = index.unwrap_or(target.tabs.len()).min(target.tabs.len());
                target.tabs.insert(at, TabData { preview: false, ..tab });
            }
        }
        target.active = Some(id.clone());
        let from = from.filter(|from| *from != to);
        if let Some(from) = from {
            self.remove(from, &[id]);
        }
        self.active_group = to;
        from
    }

    /// Moves a tab to a new group on side `side` of group `target`. The layout direction of all groups
    /// follows that side. Returns the id of the new group and the group the tab was removed from, if any.
    pub fn split_tab(&mut self, tab: TabData, from: Option<u32>, target: u32, side: SplitSide) -> (u32, Option<u32>) {
        let id = tab.id.clone();
        let from = self.source_of(&id, from);
        let index = self.groups.iter().position(|g| g.id == target).unwrap_or(self.groups.len().saturating_sub(1));
        let at = match side {
            SplitSide::Left | SplitSide::Up => index,
            SplitSide::Right | SplitSide::Down => index + 1,
        };
        let group = self.next_group;
        self.next_group += 1;
        self.groups.insert(at.min(self.groups.len()), Group { id: group, tabs: vec![TabData { preview: false, ..tab }], active: Some(id.clone()) });
        self.orientation = side.orientation();
        if let Some(from) = from {
            self.remove(from, &[id]);
        }
        self.active_group = group;
        (group, from)
    }

    /// The sides that may be used to split group `target` with tab `id` (from group `from`):
    /// `(left/right, up/down)`. Groups are laid out in one direction, so the other direction is only allowed
    /// if no other groups remain. Splitting a group that only contains that tab itself means nothing.
    pub fn split_axes(&self, target: u32, id: &str, from: Option<u32>) -> (bool, bool) {
        let from = self.source_of(id, from);
        let alone = |group: &Group| group.tabs.len() == 1 && group.contains(id);
        if from == Some(target) && self.group(target).is_some_and(alone) {
            return (false, false);
        }
        let others = self.groups.iter().filter(|g| g.id != target && !(Some(g.id) == from && alone(g))).count();
        if others == 0 {
            return (true, true);
        }
        match self.orientation {
            Orientation::Horizontal => (true, false),
            Orientation::Vertical => (false, true),
        }
    }
}

/// Where a tab is dropped over the content of a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    /// Goes into that group.
    Center,
    /// A new group on that side.
    Split(SplitSide),
}

/// The drop zone by the pointer's relative position over the group content (`x`, `y` in 0..1): near an edge
/// means splitting toward that side (if that direction is allowed), otherwise it goes into the group.
pub fn drop_zone(x: f32, y: f32, horizontal: bool, vertical: bool) -> DropZone {
    const EDGE: f32 = 0.3;
    let mut sides = Vec::new();
    if horizontal {
        sides.extend([(x, SplitSide::Left), (1. - x, SplitSide::Right)]);
    }
    if vertical {
        sides.extend([(y, SplitSide::Up), (1. - y, SplitSide::Down)]);
    }
    sides
        .into_iter()
        .filter(|(distance, _)| *distance < EDGE)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map_or(DropZone::Center, |(_, side)| DropZone::Split(side))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn file(path: &str) -> TabData {
        TabData::new("file", path, json!({ "path": path }))
    }

    fn ids(model: &Model, group: u32) -> Vec<String> {
        model.group(group).unwrap().tabs.iter().map(|t| t.title.clone()).collect()
    }

    #[test]
    fn opens_after_active_tab_and_activates() {
        let mut m = Model::default();
        m.open(file("a"), &OpenOptions::default());
        m.open(file("b"), &OpenOptions::default());
        m.activate(1, &file("a").id);
        m.open(file("c"), &OpenOptions::default());
        assert_eq!(ids(&m, 1), ["a", "c", "b"]);
        assert_eq!(m.active().unwrap().1.title, "c");
    }

    #[test]
    fn opening_existing_tab_activates_it_without_duplicating() {
        let mut m = Model::default();
        m.open(file("a"), &OpenOptions::default());
        m.open(file("b"), &OpenOptions::default());
        m.open(file("a"), &OpenOptions::default());
        assert_eq!(ids(&m, 1), ["a", "b"]);
        assert_eq!(m.active().unwrap().1.title, "a");
    }

    #[test]
    fn preview_tab_is_replaced_by_next_preview_and_pinned_on_demand() {
        let mut m = Model::default();
        m.open(file("a"), &OpenOptions::preview());
        m.open(file("b"), &OpenOptions::preview());
        assert_eq!(ids(&m, 1), ["b"]);
        assert!(m.pin(&file("b").id));
        m.open(file("c"), &OpenOptions::preview());
        assert_eq!(ids(&m, 1), ["b", "c"]);
        // Reopening a permanent tab as a preview does not make it a preview.
        m.open(file("b"), &OpenOptions::preview());
        assert!(!m.group(1).unwrap().tabs[0].preview);
        // Opening a preview tab normally makes it permanent.
        m.open(file("c"), &OpenOptions::default());
        assert!(!m.group(1).unwrap().tabs[1].preview);
    }

    #[test]
    fn to_side_creates_then_reuses_the_next_group() {
        let mut m = Model::default();
        m.open(file("a"), &OpenOptions::default());
        let g2 = m.open(file("b"), &OpenOptions::to_side());
        assert_ne!(g2, 1);
        assert_eq!(m.active_group, g2);
        m.activate_group(1);
        let again = m.open(file("c"), &OpenOptions::to_side());
        assert_eq!(again, g2);
        assert_eq!(ids(&m, g2), ["b", "c"]);
    }

    #[test]
    fn opening_a_tab_held_by_another_group_focuses_that_group() {
        let mut m = Model::default();
        m.open(file("a"), &OpenOptions::default());
        let g2 = m.open(file("b"), &OpenOptions::to_side());
        m.activate_group(1);
        assert_eq!(m.open(file("b"), &OpenOptions::default()), g2);
        assert_eq!(m.active_group, g2);
        assert_eq!(ids(&m, 1), ["a"]);
    }

    #[test]
    fn background_open_keeps_focus() {
        let mut m = Model::default();
        m.open(file("a"), &OpenOptions::default());
        m.open(file("b"), &OpenOptions { background: true, ..Default::default() });
        assert_eq!(m.active().unwrap().1.title, "a");
        assert_eq!(ids(&m, 1), ["a", "b"]);
    }

    #[test]
    fn removing_active_tab_selects_the_next_then_previous() {
        let mut m = Model::default();
        for name in ["a", "b", "c"] {
            m.open(file(name), &OpenOptions::default());
        }
        m.activate(1, &file("b").id);
        m.remove(1, &[file("b").id]);
        assert_eq!(m.active().unwrap().1.title, "c");
        m.remove(1, &[file("c").id]);
        assert_eq!(m.active().unwrap().1.title, "a");
        m.remove(1, &[file("a").id]);
        assert!(m.active().is_none());
        assert_eq!(m.groups.len(), 1);
    }

    #[test]
    fn empty_groups_are_dropped_except_the_last_one() {
        let mut m = Model::default();
        m.open(file("a"), &OpenOptions::default());
        let g2 = m.open(file("b"), &OpenOptions::to_side());
        m.remove(g2, &[file("b").id]);
        assert_eq!(m.groups.len(), 1);
        assert_eq!(m.active_group, 1);
        m.remove(1, &[file("a").id]);
        assert_eq!(m.groups.len(), 1);
    }

    #[test]
    fn update_tabs_renames_and_keeps_active() {
        let mut m = Model::default();
        m.open(file("a"), &OpenOptions::default());
        m.update_tabs(|t| (t.title == "a").then(|| file("z")));
        assert_eq!(m.active().unwrap().1.title, "z");
    }

    #[test]
    fn move_tab_reorders_within_a_group() {
        let mut m = Model::default();
        for name in ["a", "b", "c"] {
            m.open(file(name), &OpenOptions::default());
        }
        // "a" to before "c" (index 2 is computed before "a" is removed).
        m.move_tab(file("a"), Some(1), 1, Some(2));
        assert_eq!(ids(&m, 1), ["b", "a", "c"]);
        m.move_tab(file("c"), Some(1), 1, Some(0));
        assert_eq!(ids(&m, 1), ["c", "b", "a"]);
        m.move_tab(file("c"), Some(1), 1, None);
        assert_eq!(ids(&m, 1), ["c", "b", "a"], "without an index the order stays");
        assert_eq!(m.active().unwrap().1.title, "c");
    }

    #[test]
    fn move_tab_between_groups_drops_the_emptied_group() {
        let mut m = Model::default();
        m.open(file("a"), &OpenOptions::default());
        let g2 = m.open(file("b"), &OpenOptions::to_side());
        assert_eq!(m.move_tab(file("a"), Some(1), g2, Some(0)), Some(1));
        assert_eq!(m.groups.len(), 1);
        assert_eq!(ids(&m, g2), ["a", "b"]);
        assert_eq!(m.active_group, g2);
        assert_eq!(m.active().unwrap().1.title, "a");
    }

    #[test]
    fn move_tab_from_outside_opens_it_or_moves_the_open_one() {
        let mut m = Model::default();
        m.open(file("a"), &OpenOptions::default());
        let g2 = m.open(file("b"), &OpenOptions::to_side());
        // A new tab from outside the editor area.
        m.move_tab(file("x"), None, 1, Some(0));
        assert_eq!(ids(&m, 1), ["x", "a"]);
        // A tab already open in another group is moved, not duplicated.
        m.move_tab(file("b"), None, 1, None);
        assert_eq!(m.groups.len(), 1);
        assert_eq!(ids(&m, 1), ["x", "a", "b"]);
        assert!(m.group(g2).is_none());
    }

    #[test]
    fn split_tab_creates_a_group_on_the_requested_side() {
        let mut m = Model::default();
        m.open(file("a"), &OpenOptions::default());
        m.open(file("b"), &OpenOptions::default());
        let (right, from) = m.split_tab(file("b"), Some(1), 1, SplitSide::Right);
        assert_eq!(from, Some(1));
        assert_eq!(m.groups.iter().map(|g| g.id).collect::<Vec<_>>(), [1, right]);
        assert_eq!(ids(&m, 1), ["a"]);
        assert_eq!(ids(&m, right), ["b"]);
        assert_eq!(m.orientation, Orientation::Horizontal);
        assert_eq!(m.active_group, right);

        let (top, from) = m.split_tab(file("x"), None, 1, SplitSide::Up);
        assert_eq!(from, None);
        assert_eq!(m.groups.iter().map(|g| g.id).collect::<Vec<_>>(), [top, 1, right]);
        assert_eq!(m.orientation, Orientation::Vertical);
    }

    #[test]
    fn splitting_the_only_tab_of_a_group_moves_it() {
        let mut m = Model::default();
        m.open(file("a"), &OpenOptions::default());
        let g2 = m.open(file("b"), &OpenOptions::to_side());
        let (left, _) = m.split_tab(file("b"), Some(g2), 1, SplitSide::Left);
        assert_eq!(m.groups.iter().map(|g| g.id).collect::<Vec<_>>(), [left, 1]);
        assert_eq!(ids(&m, left), ["b"]);
    }

    #[test]
    fn split_axes_follow_the_single_orientation() {
        let mut m = Model::default();
        m.open(file("a"), &OpenOptions::default());
        m.open(file("b"), &OpenOptions::default());
        // One group: all sides; also for a tab from outside.
        assert_eq!(m.split_axes(1, &file("b").id, Some(1)), (true, true));
        assert_eq!(m.split_axes(1, &file("x").id, None), (true, true));
        let (g2, _) = m.split_tab(file("b"), Some(1), 1, SplitSide::Right);
        m.open(file("c"), &OpenOptions { group: Some(g2), ..Default::default() });
        // Two horizontal groups: only left/right.
        assert_eq!(m.split_axes(1, &file("c").id, Some(g2)), (true, false));
        // A group that only contains the dragged tab: it cannot be split with that tab.
        assert_eq!(m.split_axes(1, &file("a").id, Some(1)), (false, false));
        // The only tab of the other group: after the move only one group remains, so all sides are allowed.
        m.remove(g2, &[file("c").id]);
        assert_eq!(m.split_axes(1, &file("b").id, Some(g2)), (true, true));
    }

    #[test]
    fn drop_zone_prefers_the_nearest_allowed_edge() {
        assert_eq!(drop_zone(0.5, 0.5, true, true), DropZone::Center);
        assert_eq!(drop_zone(0.1, 0.5, true, true), DropZone::Split(SplitSide::Left));
        assert_eq!(drop_zone(0.95, 0.2, true, true), DropZone::Split(SplitSide::Right));
        assert_eq!(drop_zone(0.2, 0.9, true, true), DropZone::Split(SplitSide::Down));
        assert_eq!(drop_zone(0.4, 0.05, true, true), DropZone::Split(SplitSide::Up));
        // A direction that is not allowed is ignored.
        assert_eq!(drop_zone(0.5, 0.05, true, false), DropZone::Center);
        assert_eq!(drop_zone(0.05, 0.5, false, true), DropZone::Center);
        assert_eq!(drop_zone(0.05, 0.5, false, false), DropZone::Center);
    }

    #[test]
    fn cycle_wraps_around() {
        let mut m = Model::default();
        for name in ["a", "b", "c"] {
            m.open(file(name), &OpenOptions::default());
        }
        m.cycle(1);
        assert_eq!(m.active().unwrap().1.title, "a");
        m.cycle(-1);
        assert_eq!(m.active().unwrap().1.title, "c");
    }
}
