#[allow(dead_code)]
#[path = "../src/platform/policy.rs"]
mod policy;

use policy::{DesktopCommand, Hotkeys, resize_hit_test};
use std::{cell::RefCell, collections::BTreeMap};

#[test]
fn replacement_conflict_keeps_both_previous_actions_and_removes_new_binding() {
    let installed = RefCell::new(BTreeMap::new());
    let mut keys = Hotkeys::default();
    let register = |id, key| {
        installed.borrow_mut().insert(id, key);
        true
    };
    let unregister = |id| {
        installed.borrow_mut().remove(&id);
    };
    keys.apply(0x60051, 0x60052, register, unregister).unwrap();
    let original = installed.borrow().clone();
    let original_ids: Vec<_> = original.keys().copied().collect();
    assert_eq!(
        keys.command(original_ids[0]),
        Some(DesktopCommand::ToggleVisibility)
    );
    assert_eq!(keys.command(original_ids[1]), Some(DesktopCommand::Refresh));
    assert!(
        keys.apply(
            0x60078,
            0x60079,
            |id, key| {
                if key == 0x60079 {
                    false
                } else {
                    installed.borrow_mut().insert(id, key);
                    true
                }
            },
            unregister
        )
        .is_err()
    );
    assert_eq!(*installed.borrow(), original);
    keys.apply(0x60052, 0x60051, register, unregister).unwrap();
    assert_eq!(*installed.borrow(), original);
    assert_eq!(keys.command(original_ids[0]), Some(DesktopCommand::Refresh));
    keys.apply(0, 0, register, unregister).unwrap();
    assert!(installed.borrow().is_empty());
    assert_eq!(keys.command(original_ids[0]), None);
}

#[test]
fn v105_shortcut_navigation_and_numeric_contract_is_preserved() {
    let mut keys = Hotkeys::default();
    for (a, b) in [(0, 0), (0x75, 0x2004b), (0x20051, 0x40052)] {
        assert!(keys.apply(a, b, |_, _| true, |_| {}).is_ok());
    }
    for key in [
        0x4b,
        0x1004b,
        0x20012,
        0x20009,
        0x2001b,
        0x20008,
        0x2002e,
        u32::MAX,
        0x2ffff,
    ] {
        assert!(
            keys.apply(
                key,
                0,
                |_, _| panic!("invalid combination must not register"),
                |_| {}
            )
            .is_err()
        );
    }
    assert!(
        keys.apply(
            0x20051,
            0x20051,
            |_, _| panic!("duplicate must not register"),
            |_| {}
        )
        .is_err()
    );
}

#[test]
fn frameless_resize_corners_edges_and_interior_are_distinct() {
    let hit = |x, y| resize_hit_test(x, y, 550, 240, 12);
    assert_eq!(
        [hit(0, 0), hit(549, 0), hit(0, 239), hit(549, 239)],
        [13, 14, 16, 17]
    );
    assert_eq!(
        [hit(0, 120), hit(549, 120), hit(270, 0), hit(270, 239)],
        [10, 11, 12, 15]
    );
    assert_eq!(hit(270, 120), 1);
}
