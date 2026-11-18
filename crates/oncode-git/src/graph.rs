//! Penentuan jalur (lane) graph commit, bertahap mengikuti urutan `git log --topo-order`.

use crate::model::{Commit, Edge, EdgeKind, GraphRow};

/// Jumlah warna jalur yang tersedia di UI.
pub const COLOR_COUNT: u8 = 8;

#[derive(Debug, Clone)]
struct Lane {
    /// Commit yang ditunggu jalur ini.
    oid: String,
    color: u8,
}

#[derive(Debug, Default)]
pub struct GraphBuilder {
    lanes: Vec<Option<Lane>>,
    next_color: u8,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    fn take_color(&mut self) -> u8 {
        let color = self.next_color;
        self.next_color = (self.next_color + 1) % COLOR_COUNT;
        color
    }

    fn free_slot(&mut self) -> usize {
        match self.lanes.iter().position(Option::is_none) {
            Some(index) => index,
            None => {
                self.lanes.push(None);
                self.lanes.len() - 1
            }
        }
    }

    /// Memproses commit berikutnya dan menghasilkan baris graph-nya.
    pub fn push(&mut self, commit: Commit) -> GraphRow {
        let waiting: Vec<usize> = self
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, lane)| lane.as_ref().is_some_and(|l| l.oid == commit.oid))
            .map(|(i, _)| i)
            .collect();

        let (column, color) = match waiting.first() {
            Some(&col) => (col, self.lanes[col].as_ref().map(|l| l.color).unwrap_or(0)),
            None => {
                let col = self.free_slot();
                (col, self.take_color())
            }
        };

        let mut edges = Vec::new();
        for (i, lane) in self.lanes.iter().enumerate() {
            let Some(lane) = lane else { continue };
            if waiting.contains(&i) {
                edges.push(Edge { from: i as u16, to: column as u16, color: lane.color, kind: EdgeKind::Top });
            } else {
                edges.push(Edge { from: i as u16, to: i as u16, color: lane.color, kind: EdgeKind::Through });
            }
        }
        for &i in &waiting {
            self.lanes[i] = None;
        }

        for (index, parent) in commit.parents.iter().enumerate() {
            let existing = self.lanes.iter().position(|l| l.as_ref().is_some_and(|l| &l.oid == parent));
            let (target, edge_color) = match existing {
                Some(i) => (i, if index == 0 { color } else { self.lanes[i].as_ref().map(|l| l.color).unwrap_or(color) }),
                None if index == 0 => {
                    self.lanes[column] = Some(Lane { oid: parent.clone(), color });
                    (column, color)
                }
                None => {
                    // Bila induk pertama sudah punya jalur lain, kolom commit ini kosong dan dipakai ulang.
                    let slot = self.free_slot();
                    let lane_color = if slot == column { color } else { self.take_color() };
                    self.lanes[slot] = Some(Lane { oid: parent.clone(), color: lane_color });
                    (slot, lane_color)
                }
            };
            edges.push(Edge { from: column as u16, to: target as u16, color: edge_color, kind: EdgeKind::Bottom });
        }

        while matches!(self.lanes.last(), Some(None)) {
            self.lanes.pop();
        }

        GraphRow { commit, column: column as u16, color, edges }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Signature;

    fn commit(oid: &str, parents: &[&str]) -> Commit {
        let sig = Signature { name: "A".into(), email: "a@x".into(), time: 0.0 };
        Commit {
            oid: oid.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            author: sig.clone(),
            committer: sig,
            subject: oid.into(),
            refs: vec![],
        }
    }

    /// Menggambar tiap baris sebagai teks ringkas agar mudah dibandingkan.
    fn render(commits: &[(&str, &[&str])]) -> String {
        let mut builder = GraphBuilder::new();
        let mut out = String::new();
        for (oid, parents) in commits {
            let row = builder.push(commit(oid, parents));
            let mut edges: Vec<String> = row
                .edges
                .iter()
                .map(|e| {
                    let kind = match e.kind {
                        EdgeKind::Top => 'T',
                        EdgeKind::Bottom => 'B',
                        EdgeKind::Through => '|',
                    };
                    format!("{kind}{}>{}", e.from, e.to)
                })
                .collect();
            edges.sort();
            out.push_str(&format!("{oid} col={} color={} [{}]\n", row.column, row.color, edges.join(" ")));
        }
        out
    }

    #[test]
    fn linear_history_stays_in_one_lane() {
        insta::assert_snapshot!(render(&[("c", &["b"]), ("b", &["a"]), ("a", &[])]), @r"
        c col=0 color=0 [B0>0]
        b col=0 color=0 [B0>0 T0>0]
        a col=0 color=0 [T0>0]
        ");
    }

    #[test]
    fn merge_opens_and_closes_second_lane() {
        // m menggabungkan f ke b; keduanya bercabang dari a.
        insta::assert_snapshot!(render(&[("m", &["b", "f"]), ("b", &["a"]), ("f", &["a"]), ("a", &[])]), @r"
        m col=0 color=0 [B0>0 B0>1]
        b col=0 color=0 [B0>0 T0>0 |1>1]
        f col=1 color=1 [B1>0 T1>1 |0>0]
        a col=0 color=0 [T0>0]
        ");
    }

    #[test]
    fn two_branch_tips_get_separate_lanes() {
        insta::assert_snapshot!(render(&[("x", &["a"]), ("y", &["a"]), ("a", &[])]), @r"
        x col=0 color=0 [B0>0]
        y col=1 color=1 [B1>0 |0>0]
        a col=0 color=0 [T0>0]
        ");
    }

    #[test]
    fn octopus_merge_allocates_lane_per_parent() {
        insta::assert_snapshot!(render(&[("o", &["a", "b", "c"]), ("a", &["r"]), ("b", &["r"]), ("c", &["r"]), ("r", &[])]), @r"
        o col=0 color=0 [B0>0 B0>1 B0>2]
        a col=0 color=0 [B0>0 T0>0 |1>1 |2>2]
        b col=1 color=1 [B1>0 T1>1 |0>0 |2>2]
        c col=2 color=2 [B2>0 T2>2 |0>0]
        r col=0 color=0 [T0>0]
        ");
    }

    #[test]
    fn unrelated_roots_reuse_freed_lane() {
        insta::assert_snapshot!(render(&[("b", &["a"]), ("a", &[]), ("z", &["y"]), ("y", &[])]), @r"
        b col=0 color=0 [B0>0]
        a col=0 color=0 [T0>0]
        z col=0 color=1 [B0>0]
        y col=0 color=1 [T0>0]
        ");
    }

    #[test]
    fn freed_slot_is_reused_while_other_lane_continues() {
        // Jalur 0 tetap berjalan (d→c), jalur 1 selesai lalu dipakai ujung branch baru.
        insta::assert_snapshot!(render(&[("d", &["c"]), ("e", &[]), ("f", &["c"]), ("c", &[])]), @r"
        d col=0 color=0 [B0>0]
        e col=1 color=1 [|0>0]
        f col=1 color=2 [B1>0 |0>0]
        c col=0 color=0 [T0>0]
        ");
    }
}
