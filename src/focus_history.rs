//! Focus Back / Focus Forward, as upstream's `FocusHistoryModel` (scope "panes and tabs"): a
//! bounded back/forward stack of (workspace, terminal) focus positions, navigated like a browser.

use uuid::Uuid;

/// Upstream's legacy stack cap.
const MAX_HISTORY: usize = 50;
/// Upstream shows this many rows in a titlebar arrow's right-click menu.
pub const MENU_LIMIT: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub workspace: Uuid,
    pub surface: Option<Uuid>,
}

#[derive(Clone, Copy, Debug)]
struct Record {
    entry: Entry,
    /// Unix seconds, shown as "Focused HH:MM" in the arrows' menus.
    focused_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Back,
    Forward,
}

#[derive(Default)]
pub struct FocusHistory {
    records: Vec<Record>,
    /// Position of the current entry; None before anything was recorded.
    index: Option<usize>,
    /// Set while navigating, so the focus changes it causes are not recorded as new positions.
    pub suppressed: bool,
}

/// One row of an arrow's menu: where it leads, and when it was focused.
pub struct Item {
    pub index: usize,
    pub entry: Entry,
    pub focused_at: i64,
}

impl FocusHistory {
    /// Record a focus change (upstream `recordFocusInHistory`): a new position drops the forward
    /// branch, a repeat of the current one is ignored.
    pub fn record(&mut self, entry: Entry, now: i64) {
        if self.suppressed {
            return;
        }
        if let Some(index) = self.index {
            if self.records[index].entry == entry {
                return;
            }
            self.records.truncate(index + 1);
        }
        if self.records.last().is_some_and(|record| record.entry == entry) {
            self.index = Some(self.records.len() - 1);
            return;
        }
        self.records.push(Record {
            entry,
            focused_at: now,
        });
        if self.records.len() > MAX_HISTORY {
            self.records.drain(..self.records.len() - MAX_HISTORY);
        }
        self.index = Some(self.records.len() - 1);
    }

    /// Positions reachable in `direction`, nearest first. `resolve` maps an entry to where it
    /// would land today (None once its workspace is gone; a closed terminal falls back to the
    /// workspace's focused one), and an entry landing on `current` is skipped, like upstream.
    pub fn items(
        &self,
        direction: Direction,
        current: Option<Entry>,
        resolve: impl Fn(Entry) -> Option<Entry>,
    ) -> Vec<Item> {
        let Some(index) = self.index else {
            return Vec::new();
        };
        let indices: Box<dyn Iterator<Item = usize>> = match direction {
            Direction::Back => Box::new((0..index).rev()),
            Direction::Forward => Box::new(index + 1..self.records.len()),
        };
        indices
            .filter_map(|position| {
                let record = self.records[position];
                let resolved = resolve(record.entry)?;
                (Some(resolved) != current).then_some(Item {
                    index: position,
                    entry: resolved,
                    focused_at: record.focused_at,
                })
            })
            .collect()
    }

    /// Make `index` the current position after navigating to it.
    pub fn go(&mut self, index: usize) {
        if index < self.records.len() {
            self.index = Some(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(workspace: u128, surface: u128) -> Entry {
        Entry {
            workspace: Uuid::from_u128(workspace),
            surface: Some(Uuid::from_u128(surface)),
        }
    }

    /// Back and forward walk the recorded positions; a new focus after going back drops the
    /// forward branch, as in a browser.
    #[test]
    fn back_forward_and_branching() {
        let mut history = FocusHistory::default();
        let (a, b, c, d) = (entry(1, 1), entry(1, 2), entry(2, 3), entry(3, 4));
        for (time, position) in [a, b, b, c].into_iter().enumerate() {
            history.record(position, time as i64);
        }
        let same = |entry| Some(entry);
        let back = history.items(Direction::Back, Some(c), same);
        assert_eq!(back.iter().map(|item| item.entry).collect::<Vec<_>>(), [b, a]);
        assert!(history.items(Direction::Forward, Some(c), same).is_empty());

        history.go(back[0].index);
        assert_eq!(history.items(Direction::Forward, Some(b), same)[0].entry, c);
        // Navigating is not a new position.
        history.suppressed = true;
        history.record(b, 9);
        history.suppressed = false;
        assert_eq!(history.items(Direction::Forward, Some(b), same).len(), 1);

        history.record(d, 10);
        assert!(history.items(Direction::Forward, Some(d), same).is_empty());
        let back: Vec<_> = history
            .items(Direction::Back, Some(d), same)
            .into_iter()
            .map(|item| item.entry)
            .collect();
        assert_eq!(back, [b, a]);
    }

    /// Closed workspaces drop out, and positions landing where focus already is are skipped.
    #[test]
    fn unreachable_positions_are_skipped() {
        let mut history = FocusHistory::default();
        let (a, b, c) = (entry(1, 1), entry(2, 2), entry(1, 3));
        for (time, position) in [a, b, c].into_iter().enumerate() {
            history.record(position, time as i64);
        }
        let gone = Uuid::from_u128(2);
        let back = history.items(Direction::Back, Some(c), |entry| {
            (entry.workspace != gone).then_some(entry)
        });
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].entry, a);
        // A closed terminal resolves to its workspace's focused one: here, the current one.
        let back = history.items(Direction::Back, Some(c), |entry| {
            Some(if entry.workspace == Uuid::from_u128(1) { c } else { entry })
        });
        assert_eq!(back.iter().map(|item| item.entry).collect::<Vec<_>>(), [b]);
    }

    /// The stack keeps the most recent positions only.
    #[test]
    fn history_is_bounded() {
        let mut history = FocusHistory::default();
        for position in 0..(MAX_HISTORY as u128 + 10) {
            history.record(entry(1, position), position as i64);
        }
        let back = history.items(Direction::Back, None, Some);
        assert_eq!(back.len(), MAX_HISTORY - 1);
    }
}
