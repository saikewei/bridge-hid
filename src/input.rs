use crate::output::LedState;
use evdev::{Device, EventType, InputEvent, KeyCode};
use std::collections::HashSet;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
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
    keyboard_controls: Arc<Mutex<Vec<mpsc::UnboundedSender<LedState>>>>,
    current_led_state: Arc<Mutex<LedState>>,
}

impl InputManager {
    pub fn new(init_led_state: Option<LedState>) -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        // 1. 初始化空的控制器列表
        let keyboard_controls: Arc<Mutex<Vec<mpsc::UnboundedSender<LedState>>>> =
            Arc::new(Mutex::new(Vec::new()));

        // 2. 克隆 Arc 传递给后台监听任务
        let keyboard_controls_clone = Arc::clone(&keyboard_controls);
        let current_led_state = Arc::new(Mutex::new(init_led_state.unwrap_or(LedState {
            num_lock: false,
            caps_lock: false,
            scroll_lock: false,
            compose: false,
            kana: false,
        })));
        let current_led_state_clone = Arc::clone(&current_led_state);

        // 启动设备扫描和监控
        tokio::spawn(async move {
            // --- 修改点：传入 keyboard_controls_clone ---
            if let Err(e) =
                Self::monitor_devices(event_tx, keyboard_controls_clone, current_led_state_clone)
                    .await
            {
                eprintln!("Monitor Devices task failed: {:?}", e);
            }
        });

        Self {
            event_rx: event_rx,
            keyboard_controls: keyboard_controls, // 这里的 Arc 之后用于 set_all_leds
            current_led_state: Arc::new(Mutex::new(init_led_state.unwrap_or(LedState {
                num_lock: false,
                caps_lock: false,
                scroll_lock: false,
                compose: false,
                kana: false,
            }))),
        }
    }

    /// 统一控制入口：更改所有键盘的灯光
    pub async fn set_all_leds(&self, ctrl: LedState) {
        let mut controls = self.keyboard_controls.lock().unwrap();
        self.current_led_state.lock().unwrap().clone_from(&ctrl);
        // 发送指令并移除已失效的设备连接
        controls.retain(|tx| tx.send(ctrl.clone()).is_ok());
    }

    async fn monitor_devices(
        tx: mpsc::UnboundedSender<InputReport>,
        keyboard_controls: Arc<Mutex<Vec<mpsc::UnboundedSender<LedState>>>>,
        current_led_state: Arc<Mutex<LedState>>,
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
                                    let mut led_rx_to_pass = None;

                                    // 如果是键盘，创建 LED 控制通道
                                    if device_type == DeviceType::Keyboard {
                                        let (led_tx, led_rx) =
                                            mpsc::unbounded_channel::<LedState>();
                                        // 将 tx 存入全局列表，以便 InputManager::set_all_leds 广播
                                        keyboard_controls.lock().unwrap().push(led_tx);
                                        // 将 rx 准备好传给 monitor.run
                                        led_rx_to_pass = Some(led_rx);
                                    }
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
                                        monitor.run(tx_clone, led_rx_to_pass).await;

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
        let is_keyboard = keys.contains(KeyCode::KEY_A) && keys.contains(KeyCode::KEY_Z);

        // 真正的鼠标必须有左键和右键
        let is_mouse = keys.contains(KeyCode::BTN_LEFT) && keys.contains(KeyCode::BTN_RIGHT);

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
    async fn run(
        mut self,
        tx: mpsc::UnboundedSender<InputReport>,
        mut led_rx: Option<mpsc::UnboundedReceiver<LedState>>,
    ) {
        let mut led_handle = None;
        let device_name = self
            .device
            .name()
            .map(|n| n.to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        // 只有当设备类型是键盘时，才启用 LED 控制逻辑和 FD 克隆
        if self.device_type == DeviceType::Keyboard {
            // 1. 获取底层的系统文件描述符 (Raw FD)
            let raw_fd = self.device.as_raw_fd();

            // 2. 使用系统调用 dup 克隆描述符
            let cloned_fd = unsafe { libc::dup(raw_fd) };
            println!("Cloned FD: {}", cloned_fd);
            if cloned_fd < 0 {
                eprintln!("系统调用 dup 失败");
                return;
            }

            let fd_path = format!("/proc/self/fd/{}", cloned_fd);
            match Device::open(&fd_path) {
                Ok(mut write_device) => {
                    // 3. 任务一：异步 LED 写入任务
                    led_handle = Some(tokio::spawn(async move {
                        if let Some(mut rx) = led_rx {
                            while let Some(ctrl) = rx.recv().await {
                                let events = [
                                    InputEvent::new(
                                        evdev::EventType::LED.0,
                                        evdev::LedCode::LED_NUML.0,
                                        ctrl.num_lock as i32,
                                    ),
                                    InputEvent::new(
                                        evdev::EventType::LED.0,
                                        evdev::LedCode::LED_CAPSL.0,
                                        ctrl.caps_lock as i32,
                                    ),
                                    InputEvent::new(
                                        evdev::EventType::LED.0,
                                        evdev::LedCode::LED_SCROLLL.0,
                                        ctrl.scroll_lock as i32,
                                    ),
                                    InputEvent::new(
                                        evdev::EventType::LED.0,
                                        evdev::LedCode::LED_COMPOSE.0,
                                        ctrl.compose as i32,
                                    ),
                                    InputEvent::new(
                                        evdev::EventType::LED.0,
                                        evdev::LedCode::LED_KANA.0,
                                        ctrl.kana as i32,
                                    ),
                                ];

                                if let Err(e) = write_device.send_events(&events) {
                                    eprintln!("发送 LED 批量事件失败: {}", e);
                                    break;
                                }
                            }
                        }
                    }));
                }
                Err(e) => {
                    eprintln!("通过克隆的 FD 创建新 Device 失败: {}", e);
                    unsafe { libc::close(cloned_fd) };
                    // 即使 LED 辅助设备创建失败，我们通常也希望继续运行读取任务
                }
            }
        }

        // 任务二：阻塞读取任务 (无论是键盘还是鼠标都需要运行)
        let fetch_handle = tokio::task::spawn_blocking(move || {
            loop {
                let events: Vec<_> = match self.device.fetch_events() {
                    Ok(events) => events.collect(),
                    Err(e) => {
                        println!("Device {} disconnected: {:?}", device_name, e);
                        return;
                    }
                };
                for event in events {
                    if let Some(report) = self.process_event(event) {
                        if tx.send(report).is_err() {
                            return;
                        }
                    }
                }
            }
        });

        // 等待任务结束
        // 如果 led_handle 是 None，select! 会永远挂起在该分支，直到 fetch_handle 完成
        tokio::select! {
            res = async {
                if let Some(h) = led_handle {
                    let _ = h.await;
                } else {
                    // 如果是鼠标，让这个分支永远挂起，不触发 select
                    std::future::pending::<()>().await;
                }
            } => res,
            _ = fetch_handle => {
                // 读取任务结束（通常是拔掉设备），select 会随之退出，整个 run 函数结束
            },
        };
    }

    fn process_event(&mut self, event: evdev::InputEvent) -> Option<InputReport> {
        match self.device_type {
            DeviceType::Keyboard => self.process_keyboard_event(event),
            DeviceType::Mouse => self.process_mouse_event(event),
        }
    }

    fn process_keyboard_event(&mut self, event: evdev::InputEvent) -> Option<InputReport> {
        if event.event_type() == EventType::KEY {
            let key = KeyCode::new(event.code()); // 将原始 code 转换为 Key 枚举
            let value = event.value();

            if value == 2 {
                return None;
            } // 忽略自动重复

            let is_pressed = value == 1;
            let scancode = event.code();

            match key {
                KeyCode::KEY_LEFTCTRL => {
                    self.keyboard_state.modifiers = if is_pressed {
                        self.keyboard_state.modifiers | 0x01
                    } else {
                        self.keyboard_state.modifiers & !0x01
                    }
                }
                KeyCode::KEY_LEFTSHIFT => {
                    self.keyboard_state.modifiers = if is_pressed {
                        self.keyboard_state.modifiers | 0x02
                    } else {
                        self.keyboard_state.modifiers & !0x02
                    }
                }
                KeyCode::KEY_LEFTALT => {
                    self.keyboard_state.modifiers = if is_pressed {
                        self.keyboard_state.modifiers | 0x04
                    } else {
                        self.keyboard_state.modifiers & !0x04
                    }
                }
                KeyCode::KEY_LEFTMETA => {
                    self.keyboard_state.modifiers = if is_pressed {
                        self.keyboard_state.modifiers | 0x08
                    } else {
                        self.keyboard_state.modifiers & !0x08
                    }
                }
                KeyCode::KEY_RIGHTCTRL => {
                    self.keyboard_state.modifiers = if is_pressed {
                        self.keyboard_state.modifiers | 0x10
                    } else {
                        self.keyboard_state.modifiers & !0x10
                    }
                }
                KeyCode::KEY_RIGHTSHIFT => {
                    self.keyboard_state.modifiers = if is_pressed {
                        self.keyboard_state.modifiers | 0x20
                    } else {
                        self.keyboard_state.modifiers & !0x20
                    }
                }
                KeyCode::KEY_RIGHTALT => {
                    self.keyboard_state.modifiers = if is_pressed {
                        self.keyboard_state.modifiers | 0x40
                    } else {
                        self.keyboard_state.modifiers & !0x40
                    }
                }
                KeyCode::KEY_RIGHTMETA => {
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
        let axis = evdev::RelativeAxisCode(event.code());
        match event.event_type() {
            EventType::KEY => {
                let key = KeyCode::new(event.code());
                let is_pressed = event.value() == 1;
                match key {
                    KeyCode::BTN_LEFT => {
                        if is_pressed {
                            self.mouse_state.buttons |= 0x01;
                        } else {
                            self.mouse_state.buttons &= !0x01;
                        }
                    }
                    KeyCode::BTN_RIGHT => {
                        if is_pressed {
                            self.mouse_state.buttons |= 0x02;
                        } else {
                            self.mouse_state.buttons &= !0x02;
                        }
                    }
                    KeyCode::BTN_MIDDLE => {
                        if is_pressed {
                            self.mouse_state.buttons |= 0x04;
                        } else {
                            self.mouse_state.buttons &= !0x04;
                        }
                    }
                    _ => {}
                }
            }
            EventType::RELATIVE => match axis {
                evdev::RelativeAxisCode::REL_HWHEEL => {
                    self.mouse_state.x_delta = event.value() as i16
                }
                evdev::RelativeAxisCode::REL_Y => self.mouse_state.y_delta = event.value() as i16,
                evdev::RelativeAxisCode::REL_WHEEL => {
                    self.mouse_state.wheel_delta = event.value() as i8
                }
                _ => {}
            },
            EventType::SYNCHRONIZATION => {
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
        let mut manager = InputManager::new(None);

        while let Some(report) = manager.next_event().await {
            println!("Input report: {:?}", report);
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_set_all_leds() {
        println!("Starting LED control test. Please observe keyboard LEDs...");
        let manager = InputManager::new(None);
        let led_state_1 = LedState {
            num_lock: true,
            caps_lock: false,
            scroll_lock: true,
            compose: false,
            kana: false,
        };

        let led_state_2 = LedState {
            num_lock: false,
            caps_lock: true,
            scroll_lock: false,
            compose: false,
            kana: false,
        };
        for _ in 0..100 {
            manager.set_all_leds(led_state_1.clone()).await;
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            manager.set_all_leds(led_state_2.clone()).await;
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
        println!("Sent LED state to all keyboards.");
    }
}
