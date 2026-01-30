use evdev::{Device, EventType, InputEventKind, Key};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum InputReport {
    Keyboard {
        modifiers: u8,
        keys: Vec<u8>,
    },
    Mouse {
        buttons: u8,
        x: i16,
        y: i16,
        wheel: i8,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeviceType {
    Keyboard,
    Mouse,
}

struct DeviceMonitor {
    device: Device,
    device_type: DeviceType,
    keyboard_state: KeyboardState,
    mouse_state: MouseState,
}

#[derive(Default)]
struct KeyboardState {
    modifiers: u8,
    pressed_keys: Vec<u8>,
}

#[derive(Default)]
struct MouseState {
    buttons: u8,
    x_delta: i16,
    y_delta: i16,
    wheel_delta: i8,
}

pub struct InputManager {
    event_rx: mpsc::UnboundedReceiver<InputReport>,
}

impl InputManager {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        // 启动设备扫描和监控
        tokio::spawn(async move {
            // 在这里捕获可能导致的 panic，防止后台扫描任务彻底消失
            if let Err(e) = Self::monitor_devices(event_tx).await {
                eprintln!("Monitor Devices task failed: {:?}", e);
            }
        });

        Self { event_rx }
    }

    async fn monitor_devices(
        tx: mpsc::UnboundedSender<InputReport>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use tokio::time::{Duration, sleep};
        let active_monitors = Arc::new(Mutex::new(HashSet::<String>::new()));

        loop {
            // 用 try_read_dir 防止 IO 异常导致整个 loop 退出
            if let Ok(paths) = std::fs::read_dir("/dev/input") {
                for path in paths.flatten() {
                    let path_buf = path.path();
                    let path_str = path_buf.to_string_lossy().to_string();

                    if path_str.contains("event") {
                        let already_monitored = active_monitors.lock().unwrap().contains(&path_str);

                        if !already_monitored {
                            // 尝试打开设备
                            if let Ok(device) = Device::open(&path_buf) {
                                if let Some(device_type) = Self::detect_device_type(&device) {
                                    active_monitors.lock().unwrap().insert(path_str.clone());

                                    let tx_clone = tx.clone();
                                    let path_id = path_str.clone();
                                    let active_monitors_clone = Arc::clone(&active_monitors);

                                    tokio::spawn(async move {
                                        let monitor = DeviceMonitor {
                                            device,
                                            device_type,
                                            keyboard_state: KeyboardState::default(),
                                            mouse_state: MouseState::default(),
                                        };

                                        println!("Started monitoring: {}", path_id);
                                        monitor.run(tx_clone).await;

                                        // 线程退出（报错或断开）后清理占位符
                                        active_monitors_clone.lock().unwrap().remove(&path_id);
                                        println!("Stopped monitoring: {}", path_id);
                                    });
                                }
                            }
                        }
                    }
                }
            }
            // 扫描间隔
            sleep(Duration::from_secs(1)).await;
        }
    }

    fn detect_device_type(device: &Device) -> Option<DeviceType> {
        let keys = device.supported_keys()?;

        // 真正的键盘必须能打出 A 和 Z
        let is_keyboard = keys.contains(Key::KEY_A) && keys.contains(Key::KEY_Z);

        // 真正的鼠标必须有左键和右键
        let is_mouse = keys.contains(Key::BTN_LEFT) && keys.contains(Key::BTN_RIGHT);

        if is_keyboard {
            Some(DeviceType::Keyboard)
        } else if is_mouse {
            Some(DeviceType::Mouse)
        } else {
            None
        }
    }

    pub async fn next_event(&mut self) -> Option<InputReport> {
        self.event_rx.recv().await
    }
}

impl DeviceMonitor {
    async fn run(mut self, tx: mpsc::UnboundedSender<InputReport>) {
        // 使用 spawn_blocking 将这个 self 移动到专门处理阻塞 IO 的线程池中
        let _ = tokio::task::spawn_blocking(move || {
            loop {
                // 这里是同步阻塞点，但它不再卡住 Tokio 的异步线程
                let events: Vec<_> = match self.device.fetch_events() {
                    Ok(events) => events.collect(),
                    Err(_) => {
                        println!("Device disconnected");
                        return; // 设备拔出，退出线程
                    }
                };
                for event in events {
                    if let Some(report) = self.process_event(event) {
                        if tx.send(report).is_err() {
                            return; // Channel 关闭，退出线程
                        }
                    }
                }
            }
        })
        .await;
    }

    fn process_event(&mut self, event: evdev::InputEvent) -> Option<InputReport> {
        match self.device_type {
            DeviceType::Keyboard => self.process_keyboard_event(event),
            DeviceType::Mouse => self.process_mouse_event(event),
        }
    }

    fn process_keyboard_event(&mut self, event: evdev::InputEvent) -> Option<InputReport> {
        if let InputEventKind::Key(key) = event.kind() {
            let scancode = key.code();
            let value = event.value();

            // 0 = 松开, 1 = 按下, 2 = 重复
            if value == 2 {
                // 如果是自动重复，不需要更新状态
                return None;
            }

            let is_pressed = value == 1;

            match key {
                Key::KEY_LEFTCTRL => {
                    self.keyboard_state.modifiers = if is_pressed {
                        self.keyboard_state.modifiers | 0x01
                    } else {
                        self.keyboard_state.modifiers & !0x01
                    }
                }
                Key::KEY_LEFTSHIFT => {
                    self.keyboard_state.modifiers = if is_pressed {
                        self.keyboard_state.modifiers | 0x02
                    } else {
                        self.keyboard_state.modifiers & !0x02
                    }
                }
                Key::KEY_LEFTALT => {
                    self.keyboard_state.modifiers = if is_pressed {
                        self.keyboard_state.modifiers | 0x04
                    } else {
                        self.keyboard_state.modifiers & !0x04
                    }
                }
                Key::KEY_LEFTMETA => {
                    self.keyboard_state.modifiers = if is_pressed {
                        self.keyboard_state.modifiers | 0x08
                    } else {
                        self.keyboard_state.modifiers & !0x08
                    }
                }
                Key::KEY_RIGHTCTRL => {
                    self.keyboard_state.modifiers = if is_pressed {
                        self.keyboard_state.modifiers | 0x10
                    } else {
                        self.keyboard_state.modifiers & !0x10
                    }
                }
                Key::KEY_RIGHTSHIFT => {
                    self.keyboard_state.modifiers = if is_pressed {
                        self.keyboard_state.modifiers | 0x20
                    } else {
                        self.keyboard_state.modifiers & !0x20
                    }
                }
                Key::KEY_RIGHTALT => {
                    self.keyboard_state.modifiers = if is_pressed {
                        self.keyboard_state.modifiers | 0x40
                    } else {
                        self.keyboard_state.modifiers & !0x40
                    }
                }
                Key::KEY_RIGHTMETA => {
                    self.keyboard_state.modifiers = if is_pressed {
                        self.keyboard_state.modifiers | 0x80
                    } else {
                        self.keyboard_state.modifiers & !0x80
                    }
                }
                _ => {
                    if is_pressed {
                        if !self.keyboard_state.pressed_keys.contains(&(scancode as u8)) {
                            self.keyboard_state.pressed_keys.push(scancode as u8);
                        }
                    } else {
                        self.keyboard_state
                            .pressed_keys
                            .retain(|&k| k != scancode as u8);
                    }
                }
            }

            return Some(InputReport::Keyboard {
                modifiers: self.keyboard_state.modifiers,
                keys: self.keyboard_state.pressed_keys.clone(),
            });
        }
        None
    }

    fn process_mouse_event(&mut self, event: evdev::InputEvent) -> Option<InputReport> {
        match event.kind() {
            InputEventKind::Key(key) => {
                let is_pressed = event.value() == 1;
                match key {
                    Key::BTN_LEFT => {
                        if is_pressed {
                            self.mouse_state.buttons |= 0x01;
                        } else {
                            self.mouse_state.buttons &= !0x01;
                        }
                    }
                    Key::BTN_RIGHT => {
                        if is_pressed {
                            self.mouse_state.buttons |= 0x02;
                        } else {
                            self.mouse_state.buttons &= !0x02;
                        }
                    }
                    Key::BTN_MIDDLE => {
                        if is_pressed {
                            self.mouse_state.buttons |= 0x04;
                        } else {
                            self.mouse_state.buttons &= !0x04;
                        }
                    }
                    _ => {}
                }
            }
            InputEventKind::RelAxis(axis) => match axis {
                evdev::RelativeAxisType::REL_X => self.mouse_state.x_delta = event.value() as i16,
                evdev::RelativeAxisType::REL_Y => self.mouse_state.y_delta = event.value() as i16,
                evdev::RelativeAxisType::REL_WHEEL => {
                    self.mouse_state.wheel_delta = event.value() as i8
                }
                _ => {}
            },
            InputEventKind::Synchronization(_) => {
                let report = InputReport::Mouse {
                    buttons: self.mouse_state.buttons,
                    x: self.mouse_state.x_delta,
                    y: self.mouse_state.y_delta,
                    wheel: self.mouse_state.wheel_delta,
                };
                self.mouse_state.x_delta = 0;
                self.mouse_state.y_delta = 0;
                self.mouse_state.wheel_delta = 0;
                return Some(report);
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn test_input_manager() {
        println!("Starting InputManager test. Please provide keyboard/mouse input...");
        let mut manager = InputManager::new();

        while let Some(report) = manager.next_event().await {
            println!("Input report: {:?}", report);
        }
    }
}
