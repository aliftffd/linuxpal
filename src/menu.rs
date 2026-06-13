//! Click-action menu — the mouse-driven counterpart to the control socket.
//!
//! Tapping the pet opens this small panel; each row dispatches the same
//! actions the socket does (morning routine, launch apps, ask, quit). The
//! panel is anchored next to the mascot (caller passes its left x as `ox`).

use crate::bubble::{draw_text, fill_rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Morning,
    Terminal,
    Browser,
    Ask,
    Quit,
}

/// Rows, top to bottom. Order here is the on-screen order.
pub const ITEMS: [(MenuAction, &str); 5] = [
    (MenuAction::Morning, "morning"),
    (MenuAction::Terminal, "terminal"),
    (MenuAction::Browser, "browser"),
    (MenuAction::Ask, "ask"),
    (MenuAction::Quit, "quit"),
];

/// Panel width in px (caller uses it to place the panel left of the mascot).
pub const PANEL_W: usize = 104;

const Y: usize = 8;
const ROW_H: usize = 17;
const HEADER_H: usize = 13; // gap below the "ACTIONS" label before row 1

fn rows_top() -> usize {
    Y + HEADER_H
}

/// Total panel height for the current item count.
fn panel_h() -> usize {
    HEADER_H + ITEMS.len() * ROW_H + 5
}

/// Which row (if any) sits under a surface-local point, given panel origin `ox`.
pub fn hit_row(ox: usize, px: f64, py: f64) -> Option<usize> {
    if px < ox as f64 || px >= (ox + PANEL_W) as f64 {
        return None;
    }
    let top = rows_top() as f64;
    let bottom = top + (ITEMS.len() * ROW_H) as f64;
    if py < top || py >= bottom {
        return None;
    }
    Some(((py - top) / ROW_H as f64) as usize)
}

/// The action for a row index.
pub fn action(row: usize) -> Option<MenuAction> {
    ITEMS.get(row).map(|(a, _)| *a)
}

/// Draw the panel at origin `ox`. `highlight` lights the row under the cursor.
pub fn draw(buf: &mut [u8], stride: usize, ox: usize, highlight: Option<usize>) {
    let h = panel_h();

    // background — dark navy
    fill_rect(buf, stride, ox, Y, PANEL_W, h, 0x14, 0x1e, 0x2a, 235);
    // border — teal (1px)
    fill_rect(buf, stride, ox, Y, PANEL_W, 1, 0x2a, 0x5a, 0x50, 255);
    fill_rect(buf, stride, ox, Y + h - 1, PANEL_W, 1, 0x2a, 0x5a, 0x50, 255);
    fill_rect(buf, stride, ox, Y, 1, h, 0x2a, 0x5a, 0x50, 255);
    fill_rect(buf, stride, ox + PANEL_W - 1, Y, 1, h, 0x2a, 0x5a, 0x50, 255);

    // header label — amber
    draw_text(buf, stride, "ACTIONS", ox + 6, Y + 3, 0xe8, 0x94, 0x3c);

    let top = rows_top();
    for (i, (_, label)) in ITEMS.iter().enumerate() {
        let ry = top + i * ROW_H;
        if highlight == Some(i) {
            fill_rect(buf, stride, ox + 2, ry, PANEL_W - 4, ROW_H - 1, 0x2a, 0x5a, 0x50, 140);
        }
        draw_text(buf, stride, label, ox + 8, ry + 4, 0xe0, 0xd8, 0x98);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OX: usize = 200;

    #[test]
    fn hit_maps_rows() {
        let top = rows_top();
        for i in 0..ITEMS.len() {
            let y = (top + i * ROW_H + ROW_H / 2) as f64;
            assert_eq!(hit_row(OX, (OX + 5) as f64, y), Some(i), "row {i}");
        }
    }

    #[test]
    fn hit_rejects_outside() {
        let y = (rows_top() + 2) as f64;
        assert_eq!(hit_row(OX, (OX as f64) - 1.0, y), None); // left of panel
        assert_eq!(hit_row(OX, (OX + PANEL_W) as f64, y), None); // right of panel
        assert_eq!(hit_row(OX, (OX + 5) as f64, 0.0), None); // above rows
        let below = (rows_top() + ITEMS.len() * ROW_H + 1) as f64;
        assert_eq!(hit_row(OX, (OX + 5) as f64, below), None); // below last row
    }

    #[test]
    fn action_lookup() {
        assert_eq!(action(0), Some(MenuAction::Morning));
        assert_eq!(action(ITEMS.len() - 1), Some(MenuAction::Quit));
        assert_eq!(action(99), None);
    }
}
