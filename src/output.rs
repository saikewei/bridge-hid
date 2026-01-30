pub mod bluetooth;
pub mod usb;

use crate::input::InputReport;
use anyhow::Result;
use async_trait::async_trait;

/// 键盘修饰键
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyboardModifiers {
    pub left_ctrl: bool,
    pub left_shift: bool,
    pub left_alt: bool,
    pub left_gui: bool,
    pub right_ctrl: bool,
    pub right_shift: bool,
    pub right_alt: bool,
    pub right_gui: bool,
}

impl KeyboardModifiers {
    pub fn to_byte(&self) -> u8 {
        let mut byte = 0u8;
        if self.left_ctrl {
            byte |= 0x01;
        }
        if self.left_shift {
            byte |= 0x02;
        }
        if self.left_alt {
            byte |= 0x04;
        }
        if self.left_gui {
            byte |= 0x08;
        }
        if self.right_ctrl {
            byte |= 0x10;
        }
        if self.right_shift {
            byte |= 0x20;
        }
        if self.right_alt {
            byte |= 0x40;
        }
        if self.right_gui {
            byte |= 0x80;
        }
        byte
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LedState {
    pub num_lock: bool,
    pub caps_lock: bool,
    pub scroll_lock: bool,
    pub compose: bool,
    pub kana: bool,
}

impl LedState {
    fn from_byte(byte: u8) -> Self {
        Self {
            num_lock: (byte & 0x01) != 0,
            caps_lock: (byte & 0x02) != 0,
            scroll_lock: (byte & 0x04) != 0,
            compose: (byte & 0x08) != 0,
            kana: (byte & 0x10) != 0,
        }
    }
}

/// 鼠标按钮
#[derive(Debug, Clone, Copy, Default)]
pub struct MouseButtons {
    pub left: bool,
    pub right: bool,
    pub middle: bool,
}

impl MouseButtons {
    pub fn to_byte(&self) -> u8 {
        let mut byte = 0u8;
        if self.left {
            byte |= 0x01;
        }
        if self.right {
            byte |= 0x02;
        }
        if self.middle {
            byte |= 0x04;
        }
        byte
    }
}

/// HID 设备通用接口
#[async_trait]
pub trait HidBackend: Send + Sync {
    /// 核心方法：直接发送解析好的报告枚举
    async fn send_report(&mut self, report: InputReport) -> Result<()>;

    async fn get_led_state(&mut self) -> Result<Option<LedState>> {
        Ok(None)
    }
}

/// 该 trait 定义了键盘和鼠标的通用操作，
pub trait KeyboardHidDevice {
    // ========== 键盘操作 ==========

    /// 按下键盘按键
    fn key_press(&mut self, keycode: u8) -> Result<()>;

    /// 释放键盘按键
    fn key_release(&mut self, keycode: u8) -> Result<()>;

    /// 按下并释放按键（点击）
    fn key_tap(&mut self, keycode: u8) -> Result<()> {
        self.key_press(keycode)?;
        std::thread::sleep(std::time::Duration::from_millis(10));
        self.key_release(keycode)
    }

    /// 设置修饰键状态
    fn set_modifiers(&mut self, modifiers: KeyboardModifiers) -> Result<()>;

    /// 释放所有按键
    fn release_all_keys(&mut self) -> Result<()>;

    /// 输入字符串（仅支持基本 ASCII）
    fn type_string(&mut self, s: &str) -> Result<()> {
        for c in s.chars() {
            if let Some((keycode, shift)) = char_to_keycode(c) {
                if shift {
                    self.set_modifiers(KeyboardModifiers {
                        left_shift: true,
                        ..Default::default()
                    })?;
                }
                self.key_tap(keycode)?;
                if shift {
                    self.set_modifiers(KeyboardModifiers::default())?;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        Ok(())
    }

    /// 读取 LED 状态（如大写锁定等）
    fn read_led_state(&self) -> Result<Option<LedState>>;
}

pub trait MouseHidDevice {
    // ========== 鼠标操作 ==========

    /// 移动鼠标（相对移动）
    fn mouse_move(&mut self, x: i8, y: i8) -> Result<()>;

    /// 按下鼠标按钮
    fn mouse_button_press(&mut self, buttons: MouseButtons) -> Result<()>;

    /// 释放鼠标按钮
    fn mouse_button_release(&mut self) -> Result<()>;

    /// 鼠标点击
    fn mouse_click(&mut self, button: MouseButtons) -> Result<()> {
        self.mouse_button_press(button)?;
        std::thread::sleep(std::time::Duration::from_millis(10));
        self.mouse_button_release()
    }

    /// 鼠标滚轮
    fn mouse_scroll(&mut self, delta: i8) -> Result<()>;
}

/// 将字符转换为键码，返回 (keycode, need_shift)
fn char_to_keycode(c: char) -> Option<(u8, bool)> {
    match c {
        'a'..='z' => Some((keycodes::KEY_A + (c as u8 - b'a'), false)),
        'A'..='Z' => Some((keycodes::KEY_A + (c as u8 - b'A'), true)),
        '1'..='9' => Some((keycodes::KEY_1 + (c as u8 - b'1'), false)),
        '0' => Some((keycodes::KEY_0, false)),
        ' ' => Some((keycodes::KEY_SPACE, false)),
        '\n' => Some((keycodes::KEY_ENTER, false)),
        '\t' => Some((keycodes::KEY_TAB, false)),
        _ => None,
    }
}

/// 常用键码定义（HID Usage Tables）
pub mod keycodes {
    pub const KEY_A: u8 = 0x04;
    pub const KEY_B: u8 = 0x05;
    pub const KEY_C: u8 = 0x06;
    pub const KEY_D: u8 = 0x07;
    pub const KEY_E: u8 = 0x08;
    pub const KEY_F: u8 = 0x09;
    pub const KEY_G: u8 = 0x0A;
    pub const KEY_H: u8 = 0x0B;
    pub const KEY_I: u8 = 0x0C;
    pub const KEY_J: u8 = 0x0D;
    pub const KEY_K: u8 = 0x0E;
    pub const KEY_L: u8 = 0x0F;
    pub const KEY_M: u8 = 0x10;
    pub const KEY_N: u8 = 0x11;
    pub const KEY_O: u8 = 0x12;
    pub const KEY_P: u8 = 0x13;
    pub const KEY_Q: u8 = 0x14;
    pub const KEY_R: u8 = 0x15;
    pub const KEY_S: u8 = 0x16;
    pub const KEY_T: u8 = 0x17;
    pub const KEY_U: u8 = 0x18;
    pub const KEY_V: u8 = 0x19;
    pub const KEY_W: u8 = 0x1A;
    pub const KEY_X: u8 = 0x1B;
    pub const KEY_Y: u8 = 0x1C;
    pub const KEY_Z: u8 = 0x1D;
    pub const KEY_1: u8 = 0x1E;
    pub const KEY_2: u8 = 0x1F;
    pub const KEY_3: u8 = 0x20;
    pub const KEY_4: u8 = 0x21;
    pub const KEY_5: u8 = 0x22;
    pub const KEY_6: u8 = 0x23;
    pub const KEY_7: u8 = 0x24;
    pub const KEY_8: u8 = 0x25;
    pub const KEY_9: u8 = 0x26;
    pub const KEY_0: u8 = 0x27;
    pub const KEY_ENTER: u8 = 0x28;
    pub const KEY_ESC: u8 = 0x29;
    pub const KEY_BACKSPACE: u8 = 0x2A;
    pub const KEY_TAB: u8 = 0x2B;
    pub const KEY_SPACE: u8 = 0x2C;
    pub const KEY_MINUS: u8 = 0x2D;
    pub const KEY_EQUAL: u8 = 0x2E;
    pub const KEY_LEFT_BRACKET: u8 = 0x2F;
    pub const KEY_RIGHT_BRACKET: u8 = 0x30;
    pub const KEY_BACKSLASH: u8 = 0x31;
    pub const KEY_SEMICOLON: u8 = 0x33;
    pub const KEY_APOSTROPHE: u8 = 0x34;
    pub const KEY_GRAVE: u8 = 0x35;
    pub const KEY_COMMA: u8 = 0x36;
    pub const KEY_DOT: u8 = 0x37;
    pub const KEY_SLASH: u8 = 0x38;
    pub const KEY_CAPS_LOCK: u8 = 0x39;
    pub const KEY_F1: u8 = 0x3A;
    pub const KEY_F2: u8 = 0x3B;
    pub const KEY_F3: u8 = 0x3C;
    pub const KEY_F4: u8 = 0x3D;
    pub const KEY_F5: u8 = 0x3E;
    pub const KEY_F6: u8 = 0x3F;
    pub const KEY_F7: u8 = 0x40;
    pub const KEY_F8: u8 = 0x41;
    pub const KEY_F9: u8 = 0x42;
    pub const KEY_F10: u8 = 0x43;
    pub const KEY_F11: u8 = 0x44;
    pub const KEY_F12: u8 = 0x45;
    pub const KEY_PRINT_SCREEN: u8 = 0x46;
    pub const KEY_SCROLL_LOCK: u8 = 0x47;
    pub const KEY_PAUSE: u8 = 0x48;
    pub const KEY_INSERT: u8 = 0x49;
    pub const KEY_HOME: u8 = 0x4A;
    pub const KEY_PAGE_UP: u8 = 0x4B;
    pub const KEY_DELETE: u8 = 0x4C;
    pub const KEY_END: u8 = 0x4D;
    pub const KEY_PAGE_DOWN: u8 = 0x4E;
    pub const KEY_RIGHT_ARROW: u8 = 0x4F;
    pub const KEY_LEFT_ARROW: u8 = 0x50;
    pub const KEY_DOWN_ARROW: u8 = 0x51;
    pub const KEY_UP_ARROW: u8 = 0x52;
}

// 重新导出常用类型
pub use bluetooth::BluetoothKeyboardHidDevice;
pub use bluetooth::BluetoothMouseHidDevice;
pub use usb::UsbKeyboardHidDevice;
pub use usb::UsbMouseHidDevice;
