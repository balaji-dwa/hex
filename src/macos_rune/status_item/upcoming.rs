//! The menu bar's Labs › Upcoming meetings section.

use objc2_foundation::NSString;

use super::{STATUS_ITEM, StatusItemController};

/// Labs › Upcoming meetings in menu bar's section, refreshed on the meeting
/// tick and as the menu opens. It only formats the meeting controller's last
/// calendar read: EventKit is never touched here.
pub fn update_calendar_menu() {
    refresh_upcoming();
}

pub(super) fn refresh_upcoming() {
    let section = crate::calendar_menu::section(crate::calendar::wall_clock_ms());
    STATUS_ITEM.with(|controller| {
        let controller = controller.borrow();
        let Some(controller) = controller.as_ref() else {
            return;
        };
        let Some(rows) = section else {
            hide_upcoming(controller);
            return;
        };
        let labels = rows.iter().map(|row| row.label.clone()).collect::<Vec<_>>();
        let meetings = rows.into_iter().map(|row| row.meeting).collect::<Vec<_>>();
        let unchanged = {
            let current_labels = controller.upcoming_labels.borrow();
            let current_meetings = controller.upcoming_meetings.borrow();
            *current_labels == labels && *current_meetings == meetings
        };
        controller.upcoming_header.setHidden(false);
        controller.upcoming_separator.setHidden(false);
        controller.upcoming_empty.setHidden(!labels.is_empty());
        if unchanged {
            return;
        }
        for (index, item) in controller.upcoming_slots.iter().enumerate() {
            item.setHidden(index >= labels.len());
            if let Some(label) = labels.get(index) {
                item.setTitle(&NSString::from_str(label));
            }
        }
        // Assign after every AppKit call: no `RefCell` borrow is held across
        // one, so a menu callback can read the shown rows meanwhile.
        *controller.upcoming_labels.borrow_mut() = labels;
        *controller.upcoming_meetings.borrow_mut() = meetings;
    });
}

/// Hides the section and forgets what it showed, so showing it again writes
/// every row anew.
fn hide_upcoming(controller: &StatusItemController) {
    controller.upcoming_header.setHidden(true);
    controller.upcoming_empty.setHidden(true);
    controller.upcoming_separator.setHidden(true);
    for item in &controller.upcoming_slots {
        item.setHidden(true);
    }
    controller.upcoming_labels.borrow_mut().clear();
    controller.upcoming_meetings.borrow_mut().clear();
}
