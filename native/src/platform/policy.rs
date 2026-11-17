use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopCommand {
    ToggleVisibility,
    ShowSettings,
    AddSymbols,
    Refresh,
    ShowDetails,
    RemoveSelected,
    MoveUp,
    MoveDown,
    Exit,
}

#[derive(Default)]
pub struct Hotkeys {
    bindings: BTreeMap<u32, i32>,
    toggle: u32,
    refresh: u32,
    next_id: i32,
}

impl Hotkeys {
    /// Register additions before removing old combinations: a conflict is atomic.
    pub fn apply(
        &mut self,
        toggle: u32,
        refresh: u32,
        mut register: impl FnMut(i32, u32) -> bool,
        mut unregister: impl FnMut(i32),
    ) -> Result<(), String> {
        if !valid_shortcut(toggle) || !valid_shortcut(refresh) {
            return Err("快捷键无效；普通按键需 Ctrl 或 Alt，导航及取消键不能注册。".into());
        }
        if toggle != 0 && toggle == refresh {
            return Err("显示/隐藏与行情更新不能使用同一组快捷键。".into());
        }
        let mut additions = BTreeMap::new();
        for key in [toggle, refresh] {
            if key == 0 || self.bindings.contains_key(&key) {
                continue;
            }
            if !(0x5100..=0xbfff).contains(&self.next_id) {
                self.next_id = 0x5100;
            }
            while self
                .bindings
                .values()
                .chain(additions.values())
                .any(|id| *id == self.next_id)
            {
                self.next_id += 1;
                if self.next_id > 0xbfff {
                    self.next_id = 0x5100;
                }
            }
            let id = self.next_id;
            self.next_id += 1;
            if !register(id, key) {
                for id in additions.values() {
                    unregister(*id);
                }
                return Err("无法注册快捷键，可能已被其他程序占用。原快捷键保持不变。".into());
            }
            additions.insert(key, id);
        }
        self.bindings.retain(|key, id| {
            if *key == toggle || *key == refresh {
                true
            } else {
                unregister(*id);
                false
            }
        });
        self.bindings.extend(additions);
        self.toggle = toggle;
        self.refresh = refresh;
        Ok(())
    }
    pub fn command(&self, id: i32) -> Option<DesktopCommand> {
        if self.toggle != 0 && self.bindings.get(&self.toggle) == Some(&id) {
            Some(DesktopCommand::ToggleVisibility)
        } else if self.refresh != 0 && self.bindings.get(&self.refresh) == Some(&id) {
            Some(DesktopCommand::Refresh)
        } else {
            None
        }
    }
    pub fn clear(&mut self, mut unregister: impl FnMut(i32)) {
        for id in self.bindings.values() {
            unregister(*id);
        }
        self.bindings.clear();
        self.toggle = 0;
        self.refresh = 0;
    }
}

pub fn valid_shortcut(value: u32) -> bool {
    if value == 0 {
        return true;
    }
    if value & !0x0007ffff != 0 {
        return false;
    }
    let key = value & 0xffff;
    let known = matches!(key, 0x03 | 0x0c | 0x0d | 0x13..=0x15 | 0x17..=0x19 | 0x1c..=0x2d |
        0x2f..=0x39 | 0x41..=0x5d | 0x5f..=0x87 | 0x90..=0x91 | 0xa6..=0xb7 |
        0xba..=0xc0 | 0xdb..=0xdf | 0xe2 | 0xe5 | 0xe7 | 0xf6..=0xfe);
    known && ((0x70..=0x87).contains(&key) || value & 0x60000 != 0)
}

pub fn resize_hit_test(x: i32, y: i32, width: i32, height: i32, edge: i32) -> i32 {
    let (left, right, top, bottom) = (x < edge, x >= width - edge, y < edge, y >= height - edge);
    if top {
        if left {
            13
        } else if right {
            14
        } else {
            12
        }
    } else if bottom {
        if left {
            16
        } else if right {
            17
        } else {
            15
        }
    } else if left {
        10
    } else if right {
        11
    } else {
        1
    }
}
