use anyhow::{Ok, Result, anyhow};
use async_trait::async_trait;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File as TokioFile;
use tokio::io::AsyncWriteExt;
use usb_gadget::{Class, Config, Gadget, Id, Strings, default_udc, function::hid::Hid};

use crate::output::HidBackend;
use crate::output::InputReport;

use super::{KeyboardHidDevice, KeyboardModifiers, LedState, MouseButtons, MouseHidDevice};

/// 键盘 HID 报告描述符
const KEYBOARD_REPORT_DESC: &[u8] = &[
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x06, // Usage (Keyboard)
    0xA1, 0x01, // Collection (Application)
    // 修饰键 Input Report
    0x05, 0x07, //   Usage Page (Key Codes)
    0x19, 0xE0, //   Usage Minimum (224)
    0x29, 0xE7, //   Usage Maximum (231)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x01, //   Logical Maximum (1)
    0x75, 0x01, //   Report Size (1)
    0x95, 0x08, //   Report Count (8)
    0x81, 0x02, //   Input (Data, Variable, Absolute) - Modifier byte
    // 保留字节
    0x95, 0x01, //   Report Count (1)
    0x75, 0x08, //   Report Size (8)
    0x81, 0x01, //   Input (Constant) - Reserved byte
    // LED Output Report (新增)
    0x95, 0x05, //   Report Count (5) - 5个LED
    0x75, 0x01, //   Report Size (1)
    0x05, 0x08, //   Usage Page (LEDs)
    0x19, 0x01, //   Usage Minimum (Num Lock)
    0x29, 0x05, //   Usage Maximum (Kana)
    0x91, 0x02, //   Output (Data, Variable, Absolute) - LED report
    0x95, 0x01, //   Report Count (1)
    0x75, 0x03, //   Report Size (3)
    0x91, 0x01, //   Output (Constant) - LED padding
    // 按键数组
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
pub struct UsbKeyboardHidDevice {
    keyboard_file: Option<tokio::fs::File>,
    _registration: Arc<usb_gadget::RegGadget>,
    current_keys: [u8; 6],
    current_modifiers: KeyboardModifiers,
}

pub struct UsbMouseHidDevice {
    mouse_file: Option<tokio::fs::File>,
    current_buttons: MouseButtons,
    _registration: Arc<usb_gadget::RegGadget>,
}

/// 创建并初始化 USB HID 设备
pub fn build_usb_hid_device() -> Result<(UsbKeyboardHidDevice, UsbMouseHidDevice)> {
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

    let shared_reg = Arc::new(reg);

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

    // 1. 打开标准库文件句柄
    let std_file = OpenOptions::new()
        .write(true)
        .read(true)
        // .custom_flags(libc::O_NONBLOCK)
        .open(&keyboard_path)
        .map_err(|e| anyhow!("打开键盘设备 {} 失败: {}", keyboard_path.display(), e))?;

    // 2. 转换为异步句柄
    let keyboard_file = TokioFile::from_std(std_file);

    // 1. 打开标准库文件句柄
    let std_file = OpenOptions::new()
        .write(true)
        .read(true)
        // .custom_flags(libc::O_NONBLOCK)
        .open(&mouse_path)
        .map_err(|e| anyhow!("打开鼠标设备 {} 失败: {}", mouse_path.display(), e))?;
    // 2. 转换为异步句柄
    let mouse_file = TokioFile::from_std(std_file);

    Ok((
        UsbKeyboardHidDevice {
            keyboard_file: Some(keyboard_file),
            _registration: Arc::clone(&shared_reg),
            current_keys: [0u8; 6],
            current_modifiers: KeyboardModifiers::default(),
        },
        UsbMouseHidDevice {
            mouse_file: Some(mouse_file),
            _registration: Arc::clone(&shared_reg),
            current_buttons: MouseButtons::default(),
        },
    ))
}

#[async_trait]
impl HidBackend for UsbKeyboardHidDevice {
    async fn send_report(&mut self, report: InputReport) -> Result<()> {
        match report {
            InputReport::Keyboard { modifiers, keys } => {
                // 1. 构造标准的 8 字节键盘报告
                let mut data = [0u8; 8];
                data[0] = modifiers; // 修饰键字节
                data[1] = 0x00; // 保留字节

                // 2. 填充按键 (最多支持 6 个同时按下的普通键)
                for (i, &key) in keys.iter().take(6).enumerate() {
                    data[i + 2] = key;
                }

                // 3. 异步写入到键盘设备文件
                if let Some(ref mut file) = self.keyboard_file {
                    file.write_all(&data)
                        .await
                        .map_err(|e| anyhow!("异步发送键盘报告失败: {}", e))?;
                    file.flush().await?;
                }
            }
            InputReport::Mouse { .. } => {
                Err(anyhow!("收到鼠标报告,但当前后端仅支持键盘"))?;
            }
        }
        Ok(())
    }

    async fn get_led_state(&mut self) -> Result<Option<LedState>> {
        use tokio::io::AsyncReadExt;

        if let Some(ref mut file) = self.keyboard_file {
            let mut buf = [0u8; 1];

            // 使用 .await 挂起任务，直到内核缓冲区有数据或返回错误
            match file.read(&mut buf).await {
                std::result::Result::Ok(1) => Ok(Some(LedState::from_byte(buf[0]))),
                std::result::Result::Ok(0) => Ok(None), // EOF，通常表示设备关闭
                // Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                //     // 如果是 O_NONBLOCK 模式且没数据，Tokio 有时会直接返回这个错误
                //     Ok(None)
                // }
                Err(e) => Err(anyhow!("读取 LED 状态失败: {}", e)),
                _ => Err(anyhow!("读取了意外的字节数")),
            }
        } else {
            Ok(None)
        }
    }
}

#[async_trait]
impl HidBackend for UsbMouseHidDevice {
    async fn send_report(&mut self, report: InputReport) -> Result<()> {
        match report {
            InputReport::Mouse {
                buttons,
                x,
                y,
                wheel,
            } => {
                // 1. 构造标准的 4 字节鼠标报告
                let data = [
                    buttons,     // 按钮状态字节
                    x as u8,     // X 轴移动
                    y as u8,     // Y 轴移动
                    wheel as u8, // 滚轮移动
                ];
                // 2. 异步写入到鼠标设备文件
                if let Some(ref mut file) = self.mouse_file {
                    file.write_all(&data)
                        .await
                        .map_err(|e| anyhow!("异步发送鼠标报告失败: {}", e))?;

                    file.flush().await?;
                }
            }
            InputReport::Keyboard { .. } => {
                Err(anyhow!("收到键盘报告,但当前后端仅支持鼠标"))?;
            }
        }
        Ok(())
    }
}

/// 根据主次设备号查找 HID gadget 设备文件
fn find_hidg_device(major: u32, minor: u32) -> Result<PathBuf> {
    for i in 0..10 {
        let path = PathBuf::from(format!("/dev/hidg{}", i));
        if path.exists() {
            if let std::result::Result::Ok(metadata) = std::fs::metadata(&path) {
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

    #[tokio::test]
    #[ignore]
    async fn test_hid() {
        let (mut kb_hid_device, mut mouse_hid_device) =
            build_usb_hid_device().expect("创建 USB HID 设备失败");

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
            if let Err(e) = kb_hid_device
                .send_report(InputReport::Keyboard {
                    modifiers: 0,
                    keys: vec![*key],
                })
                .await
            {
                eprintln!("释放按键失败: {:?}", e);
            }
            if let Err(e) = kb_hid_device
                .send_report(InputReport::Keyboard {
                    modifiers: 0,
                    keys: vec![],
                })
                .await
            {
                eprintln!("释放按键失败: {:?}", e);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        std::thread::sleep(std::time::Duration::from_secs(1));
        println!("移动鼠标...");
        for _ in 0..50 {
            mouse_hid_device
                .send_report(InputReport::Mouse {
                    buttons: 0,
                    x: 0,
                    y: -5,
                    wheel: 0,
                })
                .await
                .expect("移动鼠标失败");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        mouse_hid_device
            .send_report(InputReport::Mouse {
                buttons: 0x01,
                x: 0,
                y: 0,
                wheel: 0,
            })
            .await
            .expect("鼠标点击失败");
        for _ in 0..50 {
            mouse_hid_device
                .send_report(InputReport::Mouse {
                    buttons: 0,
                    x: 0,
                    y: 0,
                    wheel: 1,
                })
                .await
                .expect("滚动鼠标失败");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_led() {
        let (mut kb_hid_device, _) = build_usb_hid_device().expect("创建 USB HID 设备失败");

        println!("等待 USB 设备枚举...");
        std::thread::sleep(std::time::Duration::from_secs(2));

        // 1. 初始化一个“上一次状态”的缓存
        let mut last_led_state: Option<LedState> = None;

        println!("等待 LED 状态变化...");
        loop {
            match kb_hid_device.get_led_state().await {
                std::result::Result::Ok(Some(new_state)) => {
                    // 2. 只有新旧状态不同，才打印并执行逻辑
                    if Some(new_state) != last_led_state {
                        println!("LED 状态发生变更: {:?}", new_state);

                        // 3. 更新缓存
                        last_led_state = Some(new_state);

                        // 在这里执行你真正的同步逻辑，例如同步给物理键盘
                        // self.input_manager.set_all_leds(new_state);
                    }
                }
                std::result::Result::Ok(None) => {}
                Err(e) => {
                    eprintln!("读取失败: {}", e);
                    break;
                }
            }
        }
    }
}
