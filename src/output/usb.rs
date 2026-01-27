use anyhow::{Result, anyhow};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use usb_gadget::{Class, Config, Gadget, Id, Strings, default_udc, function::hid::Hid};

use super::{HidDevice, KeyboardModifiers, MouseButtons};

/// 键盘 HID 报告描述符
const KEYBOARD_REPORT_DESC: &[u8] = &[
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x06, // Usage (Keyboard)
    0xA1, 0x01, // Collection (Application)
    0x05, 0x07, //   Usage Page (Key Codes)
    0x19, 0xE0, //   Usage Minimum (224)
    0x29, 0xE7, //   Usage Maximum (231)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x01, //   Logical Maximum (1)
    0x75, 0x01, //   Report Size (1)
    0x95, 0x08, //   Report Count (8)
    0x81, 0x02, //   Input (Data, Variable, Absolute) - Modifier byte
    0x95, 0x01, //   Report Count (1)
    0x75, 0x08, //   Report Size (8)
    0x81, 0x01, //   Input (Constant) - Reserved byte
    0x95, 0x06, //   Report Count (6)
    0x75, 0x08, //   Report Size (8)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x65, //   Logical Maximum (101)
    0x05, 0x07, //   Usage Page (Key Codes)
    0x19, 0x00, //   Usage Minimum (0)
    0x29, 0x65, //   Usage Maximum (101)
    0x81, 0x00, //   Input (Data, Array) - Key arrays (6 keys)
    0xC0, // End Collection
];

/// 鼠标 HID 报告描述符
const MOUSE_REPORT_DESC: &[u8] = &[
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x02, // Usage (Mouse)
    0xA1, 0x01, // Collection (Application)
    0x09, 0x01, //   Usage (Pointer)
    0xA1, 0x00, //   Collection (Physical)
    0x05, 0x09, //     Usage Page (Buttons)
    0x19, 0x01, //     Usage Minimum (1)
    0x29, 0x03, //     Usage Maximum (3)
    0x15, 0x00, //     Logical Minimum (0)
    0x25, 0x01, //     Logical Maximum (1)
    0x95, 0x03, //     Report Count (3)
    0x75, 0x01, //     Report Size (1)
    0x81, 0x02, //     Input (Data, Variable, Absolute) - Buttons
    0x95, 0x01, //     Report Count (1)
    0x75, 0x05, //     Report Size (5)
    0x81, 0x01, //     Input (Constant) - Padding
    0x05, 0x01, //     Usage Page (Generic Desktop)
    0x09, 0x30, //     Usage (X)
    0x09, 0x31, //     Usage (Y)
    0x09, 0x38, //     Usage (Wheel)
    0x15, 0x81, //     Logical Minimum (-127)
    0x25, 0x7F, //     Logical Maximum (127)
    0x75, 0x08, //     Report Size (8)
    0x95, 0x03, //     Report Count (3)
    0x81, 0x06, //     Input (Data, Variable, Relative) - X, Y, Wheel
    0xC0, //   End Collection
    0xC0, // End Collection
];

/// USB HID 键盘鼠标模拟器
pub struct UsbHidDevice {
    keyboard_file: Option<File>,
    mouse_file: Option<File>,
    _registration: usb_gadget::RegGadget,
    current_keys: [u8; 6],
    current_modifiers: KeyboardModifiers,
    current_buttons: MouseButtons,
}

impl UsbHidDevice {
    /// 创建并初始化 USB HID 设备
    pub fn new() -> Result<Self> {
        usb_gadget::remove_all().map_err(|e| anyhow!("无法移除现有 gadgets: {}", e))?;

        // 创建键盘 HID 功能
        let mut keyboard_builder = Hid::builder();
        keyboard_builder.sub_class = 1; // Boot Interface Subclass
        keyboard_builder.protocol = 1; // Keyboard
        keyboard_builder.report_desc = KEYBOARD_REPORT_DESC.to_vec();
        keyboard_builder.report_len = 8;
        let (keyboard_hid, keyboard_handle) = keyboard_builder.build();

        // 创建鼠标 HID 功能
        let mut mouse_builder = Hid::builder();
        mouse_builder.sub_class = 1; // Boot Interface Subclass
        mouse_builder.protocol = 2; // Mouse
        mouse_builder.report_desc = MOUSE_REPORT_DESC.to_vec();
        mouse_builder.report_len = 4;
        let (mouse_hid, mouse_handle) = mouse_builder.build();

        // 获取 UDC
        let udc = default_udc().map_err(|e| anyhow!("获取 UDC 失败: {}", e))?;

        // 创建 USB Gadget
        let mut gadget = Gadget::new(
            Class::new(0x00, 0x00, 0x00),
            Id::new(0x1d6b, 0x0104),
            Strings::new("Bridge HID", "Virtual Keyboard Mouse", "001"),
        );

        let mut config = Config::new("config");
        config.add_function(keyboard_handle);
        config.add_function(mouse_handle);
        gadget.add_config(config);

        // 注册并绑定
        let reg = gadget
            .bind(&udc)
            .map_err(|e| anyhow!("注册并绑定 Gadget 失败: {}", e))?;

        // 等待设备节点创建
        std::thread::sleep(std::time::Duration::from_millis(100));

        // 获取设备文件路径
        let keyboard_dev = keyboard_hid
            .device()
            .map_err(|e| anyhow!("获取键盘设备号失败: {}", e))?;
        let mouse_dev = mouse_hid
            .device()
            .map_err(|e| anyhow!("获取鼠标设备号失败: {}", e))?;

        let keyboard_path = find_hidg_device(keyboard_dev.0, keyboard_dev.1)?;
        let mouse_path = find_hidg_device(mouse_dev.0, mouse_dev.1)?;

        let keyboard_file = OpenOptions::new()
            .write(true)
            .open(&keyboard_path)
            .map_err(|e| anyhow!("打开键盘设备 {} 失败: {}", keyboard_path.display(), e))?;

        let mouse_file = OpenOptions::new()
            .write(true)
            .open(&mouse_path)
            .map_err(|e| anyhow!("打开鼠标设备 {} 失败: {}", mouse_path.display(), e))?;

        Ok(Self {
            keyboard_file: Some(keyboard_file),
            mouse_file: Some(mouse_file),
            _registration: reg,
            current_keys: [0u8; 6],
            current_modifiers: KeyboardModifiers::default(),
            current_buttons: MouseButtons::default(),
        })
    }

    /// 发送键盘报告
    fn send_keyboard_report(&mut self) -> Result<()> {
        let report = [
            self.current_modifiers.to_byte(),
            0x00, // Reserved
            self.current_keys[0],
            self.current_keys[1],
            self.current_keys[2],
            self.current_keys[3],
            self.current_keys[4],
            self.current_keys[5],
        ];

        if let Some(ref mut file) = self.keyboard_file {
            file.write_all(&report)
                .map_err(|e| anyhow!("发送键盘报告失败: {}", e))?;
            file.flush()?;
        }
        Ok(())
    }

    /// 发送鼠标报告
    fn send_mouse_report(&mut self, x: i8, y: i8, wheel: i8) -> Result<()> {
        let report = [
            self.current_buttons.to_byte(),
            x as u8,
            y as u8,
            wheel as u8,
        ];

        if let Some(ref mut file) = self.mouse_file {
            file.write_all(&report)
                .map_err(|e| anyhow!("发送鼠标报告失败: {}", e))?;
            file.flush()?;
        }
        Ok(())
    }
}

impl HidDevice for UsbHidDevice {
    fn key_press(&mut self, keycode: u8) -> Result<()> {
        for key in &mut self.current_keys {
            if *key == 0 {
                *key = keycode;
                break;
            }
        }
        self.send_keyboard_report()
    }

    fn key_release(&mut self, keycode: u8) -> Result<()> {
        for key in &mut self.current_keys {
            if *key == keycode {
                *key = 0;
                break;
            }
        }
        self.send_keyboard_report()
    }

    fn set_modifiers(&mut self, modifiers: KeyboardModifiers) -> Result<()> {
        self.current_modifiers = modifiers;
        self.send_keyboard_report()
    }

    fn release_all_keys(&mut self) -> Result<()> {
        self.current_keys = [0u8; 6];
        self.current_modifiers = KeyboardModifiers::default();
        self.send_keyboard_report()
    }

    fn mouse_move(&mut self, x: i8, y: i8) -> Result<()> {
        self.send_mouse_report(x, y, 0)
    }

    fn mouse_button_press(&mut self, buttons: MouseButtons) -> Result<()> {
        self.current_buttons = buttons;
        self.send_mouse_report(0, 0, 0)
    }

    fn mouse_button_release(&mut self) -> Result<()> {
        self.current_buttons = MouseButtons::default();
        self.send_mouse_report(0, 0, 0)
    }

    fn mouse_scroll(&mut self, delta: i8) -> Result<()> {
        self.send_mouse_report(0, 0, delta)
    }
}

/// 根据主次设备号查找 HID gadget 设备文件
fn find_hidg_device(major: u32, minor: u32) -> Result<PathBuf> {
    for i in 0..10 {
        let path = PathBuf::from(format!("/dev/hidg{}", i));
        if path.exists() {
            if let Ok(metadata) = std::fs::metadata(&path) {
                use std::os::unix::fs::MetadataExt;
                let dev = metadata.rdev();
                let dev_major = ((dev >> 8) & 0xfff) as u32;
                let dev_minor = (dev & 0xff) as u32;
                if dev_major == major && dev_minor == minor {
                    return Ok(path);
                }
            }
        }
    }
    Err(anyhow!("未找到设备 {}:{}", major, minor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::keycodes;

    #[test]
    #[ignore]
    fn test_hid() {
        let mut hid_device = UsbHidDevice::new().expect("创建 USB HID 设备失败");

        println!("等待 USB 设备枚举...");
        std::thread::sleep(std::time::Duration::from_secs(2));

        let keys = [
            keycodes::KEY_H,
            keycodes::KEY_E,
            keycodes::KEY_L,
            keycodes::KEY_L,
            keycodes::KEY_O,
        ];

        for (i, key) in keys.iter().enumerate() {
            println!("发送按键 {}/{}...", i + 1, keys.len());
            if let Err(e) = hid_device.key_tap(*key) {
                eprintln!("按键失败: {}", e);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        std::thread::sleep(std::time::Duration::from_secs(1));
        println!("移动鼠标...");
        for _ in 0..50 {
            hid_device.mouse_move(0, -5).expect("移动鼠标失败");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        hid_device
            .mouse_click(MouseButtons {
                left: true,
                right: false,
                middle: false,
            })
            .expect("鼠标点击失败");
        hid_device.mouse_scroll(5).expect("鼠标滚轮失败");
    }
}
