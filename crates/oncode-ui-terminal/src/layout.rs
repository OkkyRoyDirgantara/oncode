//! Susunan terminal di panel: kelompok-kelompok terminal yang tampil berdampingan (split), seperti
//! VS Code. Panel menampilkan satu kelompok, yaitu kelompok terminal aktif. Murni (tanpa GPUI).

/// Id sesi terminal, unik di seluruh aplikasi.
pub type SessionId = u64;

/// Tempat terminal ditaruh di panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelDrop {
    /// Kelompok sendiri, sebelum kelompok yang memuat terminal ini (`None` = paling akhir).
    Alone(Option<SessionId>),
    /// Berdampingan dengan terminal ini: di kanannya (`true`) atau kirinya (`false`).
    Beside(SessionId, bool),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    pub groups: Vec<Vec<SessionId>>,
    /// Terminal aktif; kelompoknya yang tampil di panel.
    pub active: Option<SessionId>,
}

impl Layout {
    pub fn contains(&self, id: SessionId) -> bool {
        self.group_of(id).is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// Indeks kelompok yang memuat terminal ini.
    pub fn group_of(&self, id: SessionId) -> Option<usize> {
        self.groups.iter().position(|group| group.contains(&id))
    }

    /// Semua terminal menurut urutan daftar (kelompok demi kelompok).
    pub fn order(&self) -> Vec<SessionId> {
        self.groups.iter().flatten().copied().collect()
    }

    /// Kelompok terminal aktif.
    pub fn active_group(&self) -> &[SessionId] {
        self.active.and_then(|id| self.group_of(id)).map_or(&[], |index| &self.groups[index])
    }

    /// Mencabut terminal. Kelompok yang kosong dibuang; bila terminal itu aktif, penggantinya
    /// tetangganya di kelompok yang sama, atau kelompok berikutnya/sebelumnya.
    pub fn remove(&mut self, id: SessionId) -> bool {
        let Some(group) = self.group_of(id) else { return false };
        let index = self.groups[group].iter().position(|x| *x == id).unwrap_or(0);
        self.groups[group].remove(index);
        let replacement = if self.groups[group].is_empty() {
            self.groups.remove(group);
            self.groups.get(group).or_else(|| self.groups.get(group.wrapping_sub(1))).and_then(|g| g.first().copied())
        } else {
            Some(self.groups[group][index.min(self.groups[group].len() - 1)])
        };
        if self.active == Some(id) {
            self.active = replacement;
        }
        true
    }

    /// Menaruh terminal (memindahkannya bila sudah ada di panel) lalu menjadikannya aktif.
    /// Menaruh terminal di dekat dirinya sendiri tidak mengubah apa pun.
    pub fn place(&mut self, id: SessionId, drop: PanelDrop) {
        let target = match drop {
            PanelDrop::Alone(target) => target,
            PanelDrop::Beside(target, _) => Some(target),
        };
        if target == Some(id) && self.contains(id) {
            self.active = Some(id);
            return;
        }
        self.remove(id);
        match drop {
            PanelDrop::Alone(target) => {
                let at = target.and_then(|t| self.group_of(t)).unwrap_or(self.groups.len());
                self.groups.insert(at, vec![id]);
            }
            PanelDrop::Beside(target, after) => match self.group_of(target) {
                Some(group) => {
                    let index = self.groups[group].iter().position(|x| *x == target).unwrap_or(0) + usize::from(after);
                    self.groups[group].insert(index, id);
                }
                None => self.groups.push(vec![id]),
            },
        }
        self.active = Some(id);
    }

    /// Mengeluarkan terminal dari kelompoknya menjadi kelompok sendiri, tepat setelah kelompok itu.
    pub fn unsplit(&mut self, id: SessionId) {
        let Some(group) = self.group_of(id) else { return };
        if self.groups[group].len() < 2 {
            return;
        }
        self.groups[group].retain(|x| *x != id);
        self.groups.insert(group + 1, vec![id]);
        self.active = Some(id);
    }

    /// Terminal berikut (`delta` 1) atau sebelumnya (-1) dari yang aktif, berputar.
    pub fn cycle(&self, delta: isize) -> Option<SessionId> {
        let order = self.order();
        if order.is_empty() {
            return None;
        }
        let index = self.active.and_then(|id| order.iter().position(|x| *x == id)).unwrap_or(0) as isize;
        Some(order[(index + delta).rem_euclid(order.len() as isize) as usize])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(groups: &[&[SessionId]], active: SessionId) -> Layout {
        Layout { groups: groups.iter().map(|g| g.to_vec()).collect(), active: Some(active) }
    }

    #[test]
    fn placing_beside_joins_the_target_group() {
        let mut l = layout(&[&[1], &[2]], 1);
        l.place(3, PanelDrop::Beside(1, true));
        assert_eq!(l, layout(&[&[1, 3], &[2]], 3));
        l.place(2, PanelDrop::Beside(1, false));
        assert_eq!(l, layout(&[&[2, 1, 3]], 2));
        assert_eq!(l.active_group(), [2, 1, 3]);
    }

    #[test]
    fn placing_alone_moves_out_of_a_split() {
        let mut l = layout(&[&[1, 2], &[3]], 2);
        l.place(2, PanelDrop::Alone(Some(1)));
        assert_eq!(l, layout(&[&[2], &[1], &[3]], 2));
        l.place(2, PanelDrop::Alone(None));
        assert_eq!(l, layout(&[&[1], &[3], &[2]], 2));
        // Terminal dari luar panel (area editor).
        l.place(9, PanelDrop::Alone(Some(3)));
        assert_eq!(l, layout(&[&[1], &[9], &[3], &[2]], 9));
    }

    #[test]
    fn placing_next_to_itself_only_activates() {
        let mut l = layout(&[&[1, 2]], 1);
        l.place(2, PanelDrop::Beside(2, true));
        l.place(2, PanelDrop::Alone(Some(2)));
        assert_eq!(l, layout(&[&[1, 2]], 2));
    }

    #[test]
    fn removing_picks_a_neighbour_and_drops_empty_groups() {
        let mut l = layout(&[&[1, 2, 3], &[4]], 3);
        assert!(l.remove(3));
        assert_eq!(l, layout(&[&[1, 2], &[4]], 2));
        l.active = Some(4);
        l.remove(4);
        assert_eq!(l, layout(&[&[1, 2]], 1));
        l.remove(1);
        l.remove(2);
        assert!(l.is_empty());
        assert_eq!(l.active, None);
        assert!(!l.remove(7));
    }

    #[test]
    fn removing_an_inactive_terminal_keeps_the_active_one() {
        let mut l = layout(&[&[1], &[2], &[3]], 3);
        l.remove(1);
        assert_eq!(l, layout(&[&[2], &[3]], 3));
    }

    #[test]
    fn unsplit_puts_the_terminal_right_after_its_group() {
        let mut l = layout(&[&[1, 2], &[3]], 1);
        l.unsplit(1);
        assert_eq!(l, layout(&[&[2], &[1], &[3]], 1));
        l.unsplit(3);
        assert_eq!(l, layout(&[&[2], &[1], &[3]], 1), "terminal sendirian tidak berubah");
    }

    #[test]
    fn cycle_walks_the_list_order() {
        let l = layout(&[&[1, 2], &[3]], 2);
        assert_eq!(l.cycle(1), Some(3));
        assert_eq!(l.cycle(-1), Some(1));
        assert_eq!(layout(&[&[1, 2], &[3]], 3).cycle(1), Some(1));
        assert_eq!(Layout::default().cycle(1), None);
    }
}
