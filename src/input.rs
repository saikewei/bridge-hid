use crate::output::LedState;
use anyhow::Context;
use evdev::{Device, EventType, InputEvent, KeyCode};
use log::{debug, error, info, trace, warn};
use std::collections::HashSet;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
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

static SYN_COUNT: AtomicU64 = AtomicU64::new(0);
static SYN_LAST: OnceLock<Mutex<Instant>> = OnceLock::new();
static LAST_CALL: OnceLock<Mutex<Instant>> = OnceLock::new();

fn record_syn_rate() {
    SYN_COUNT.fetch_add(1, Ordering::Relaxed);

    let lock = SYN_LAST.get_or_init(|| Mutex::new(Instant::now()));
    let mut last = lock.lock().unwrap();

    if last.elapsed() >= Duration::from_secs(1) {
        let count = SYN_COUNT.swap(0, Ordering::Relaxed);
        trace!("SYN_REPORT rate = {}", count);
        *last = Instant::now();
    }
}

fn elapsed_since_last_call_ms() {
    // 第一次调用时初始化
    let lock = LAST_CALL.get_or_init(|| Mutex::new(Instant::now()));

    // 获取锁
    let mut last = lock.lock().unwrap();

    // 计算距离上次调用的时间
    let elapsed = last.elapsed().as_millis();

    // 更新为当前时间
    *last = Instant::now();

    if elapsed > 10 {
        warn!(
            "Warning: Long delay between SYN_REPORT events: {} ms",
            elapsed
        );
    }
}

struct DeviceMonitor {
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
    dirty: bool,
}

pub struct LedHandle {
    keyboard_controls: Arc<Mutex<Vec<mpsc::UnboundedSender<LedState>>>>,
    current_led_state: Arc<Mutex<LedState>>,
}

impl LedHandle {
    pub fn new() -> Self {
        Self {
            keyboard_controls: Arc::new(Mutex::new(Vec::new())),
            current_led_state: Arc::new(Mutex::new(LedState::default())),
        }
    }

    pub async fn set_leds(&self, ctrl: &LedState) {
        let mut controls = self.keyboard_controls.lock().unwrap();
        self.current_led_state.lock().unwrap().clone_from(&ctrl);
        // 发送指令并移除已失效的设备连接
        controls.retain(|tx| tx.send(ctrl.clone()).is_ok());
    }
}

pub struct InputManager {
    event_rx: mpsc::UnboundedReceiver<InputReport>,
    pub led_handle: Option<LedHandle>,
}

impl InputManager {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let led_handle = LedHandle::new();
        let keyboard_controls = Arc::clone(&led_handle.keyboard_controls);
        let current_led_state = Arc::clone(&led_handle.current_led_state);

        // 启动设备扫描和监控
        tokio::spawn(async move {
            if let Err(e) =
                Self::monitor_devices(event_tx, keyboard_controls, current_led_state).await
            {
                error!("Monitor Devices task failed: {}", e);
            }
        });

        Self {
            event_rx: event_rx,
            led_handle: Some(led_handle),
        }
    }

    async fn monitor_devices(
        tx: mpsc::UnboundedSender<InputReport>,
        keyboard_controls: Arc<Mutex<Vec<mpsc::UnboundedSender<LedState>>>>,
        current_led_state: Arc<Mutex<LedState>>,
    ) -> anyhow::Result<()> {
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
                            if let Ok(mut device) = Device::open(&path_buf) {
                                if let Some(device_type) = Self::detect_device_type(&device) {
                                    active_monitors.lock().unwrap().insert(path_str.clone());

                                    let tx_clone = tx.clone();
                                    let mut led_rx_to_pass = None;
                                    let mut current_led_state_clone = None;

                                    // 如果是键盘，创建 LED 控制通道
                                    if device_type == DeviceType::Keyboard {
                                        device.grab().context("独占键盘设备失败")?;
                                        let (led_tx, led_rx) =
                                            mpsc::unbounded_channel::<LedState>();
                                        // 将 tx 存入全局列表，以便 InputManager::set_all_leds 广播
                                        keyboard_controls.lock().unwrap().push(led_tx);
                                        // 将 rx 准备好传给 monitor.run
                                        led_rx_to_pass = Some(led_rx);
                                        current_led_state_clone = Some(
                                            current_led_state
                                                .lock()
                                                .map(|guard| guard.clone())
                                                .unwrap_or_default(),
                                        );

                                        debug!(
                                            "current_led_state_clone: {:?}",
                                            current_led_state_clone
                                        );
                                    }
                                    let path_id = path_str.clone();
                                    let active_monitors_clone = Arc::clone(&active_monitors);

                                    tokio::spawn(async move {
                                        let monitor = DeviceMonitor {
                                            device_type,
                                            keyboard_state: KeyboardState::default(),
                                            mouse_state: MouseState::default(),
                                        };

                                        info!("Started monitoring: {}", path_id);
                                        monitor.run(tx_clone, led_rx_to_pass, device).await;

                                        active_monitors_clone.lock().unwrap().remove(&path_id);
                                        info!("Stopped monitoring: {}", path_id);
                                    });

                                    // 发送当前 LED 状态以同步新连接的键盘
                                    if let Some(ctrl) = current_led_state_clone {
                                        if let Some(last_tx) =
                                            keyboard_controls.lock().unwrap().last()
                                        {
                                            let _ = last_tx.send(ctrl);
                                        }
                                    }
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
        mut device: Device,
    ) {
        let mut led_handle = None;
        let device_name = device
            .name()
            .map(|n| n.to_string())
            .unwrap_or_else(|| "Unknown".to_string());
        debug!("Device name: {}", device_name);

        if self.device_type == DeviceType::Keyboard {
            let raw_fd = device.as_raw_fd();

            let cloned_fd = unsafe { libc::dup(raw_fd) };
            debug!("Cloned FD: {}", cloned_fd);
            if cloned_fd < 0 {
                error!("系统调用 dup 失败");
                return;
            }

            let fd_path = format!("/proc/self/fd/{}", cloned_fd);
            match Device::open(&fd_path)
                .with_context(|| format!("打开克隆 FD 设备失败: {}", fd_path))
            {
                Ok(mut write_device) => {
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
                                    error!("发送 LED 批量事件失败: {}", e);
                                    break;
                                }
                            }
                        }
                    }));
                }
                Err(e) => {
                    error!("通过克隆的 FD 创建新 Device 失败: {}", e);
                    unsafe { libc::close(cloned_fd) };
                }
            }
        }

        let fetch_handle = tokio::task::spawn_blocking(move || {
            loop {
                match device.fetch_events() {
                    Ok(events) => {
                        for event in events {
                            if let Some(report) = self.process_event(event) {
                                if tx.send(report).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("读取事件失败: {}", e);
                        return;
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
            let scancode = evdev_to_hid(key);

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
                        if !self
                            .keyboard_state
                            .pressed_keys
                            .contains(&(scancode.expect("键码错误")))
                        {
                            self.keyboard_state
                                .pressed_keys
                                .push(scancode.expect("键码错误"));
                        }
                    } else {
                        self.keyboard_state
                            .pressed_keys
                            .retain(|&k| k != scancode.expect("键码错误"));
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
                    _ => return None,
                }
                // 按钮变化需要标记，但也等 SYN_REPORT 一起发
                self.mouse_state.dirty = true;
            }

            EventType::RELATIVE => {
                match axis {
                    evdev::RelativeAxisCode::REL_X => {
                        self.mouse_state.x_delta += event.value() as i16;
                    }
                    evdev::RelativeAxisCode::REL_Y => {
                        self.mouse_state.y_delta += event.value() as i16;
                    }
                    evdev::RelativeAxisCode::REL_WHEEL => {
                        self.mouse_state.wheel_delta += event.value() as i8;
                    }
                    _ => return None,
                }
                self.mouse_state.dirty = true;
            }

            // 只在 SYN_REPORT 时发送完整报告
            EventType::SYNCHRONIZATION => {
                if self.mouse_state.dirty {
                    // record_syn_rate();
                    // elapsed_since_last_call_ms();
                    self.mouse_state.dirty = false;
                    return Some(self.build_mouse_report());
                }
            }

            _ => {}
        }

        None
    }

    fn build_mouse_report(&mut self) -> InputReport {
        let report = InputReport::Mouse {
            buttons: self.mouse_state.buttons,
            x: self.mouse_state.x_delta,
            y: self.mouse_state.y_delta,
            wheel: self.mouse_state.wheel_delta,
        };

        // ⭐发完立刻清空 delta
        self.mouse_state.x_delta = 0;
        self.mouse_state.y_delta = 0;
        self.mouse_state.wheel_delta = 0;

        report
    }
}

fn evdev_to_hid(code: KeyCode) -> Option<u8> {
    Some(match code {
        // ----- 字母 -----
        KeyCode::KEY_A => 0x04,
        KeyCode::KEY_B => 0x05,
        KeyCode::KEY_C => 0x06,
        KeyCode::KEY_D => 0x07,
        KeyCode::KEY_E => 0x08,
        KeyCode::KEY_F => 0x09,
        KeyCode::KEY_G => 0x0A,
        KeyCode::KEY_H => 0x0B,
        KeyCode::KEY_I => 0x0C,
        KeyCode::KEY_J => 0x0D,
        KeyCode::KEY_K => 0x0E,
        KeyCode::KEY_L => 0x0F,
        KeyCode::KEY_M => 0x10,
        KeyCode::KEY_N => 0x11,
        KeyCode::KEY_O => 0x12,
        KeyCode::KEY_P => 0x13,
        KeyCode::KEY_Q => 0x14,
        KeyCode::KEY_R => 0x15,
        KeyCode::KEY_S => 0x16,
        KeyCode::KEY_T => 0x17,
        KeyCode::KEY_U => 0x18,
        KeyCode::KEY_V => 0x19,
        KeyCode::KEY_W => 0x1A,
        KeyCode::KEY_X => 0x1B,
        KeyCode::KEY_Y => 0x1C,
        KeyCode::KEY_Z => 0x1D,

        // ----- 数字行 -----
        KeyCode::KEY_1 => 0x1E,
        KeyCode::KEY_2 => 0x1F,
        KeyCode::KEY_3 => 0x20,
        KeyCode::KEY_4 => 0x21,
        KeyCode::KEY_5 => 0x22,
        KeyCode::KEY_6 => 0x23,
        KeyCode::KEY_7 => 0x24,
        KeyCode::KEY_8 => 0x25,
        KeyCode::KEY_9 => 0x26,
        KeyCode::KEY_0 => 0x27,

        // ----- 基本控制 -----
        KeyCode::KEY_ENTER => 0x28,
        KeyCode::KEY_ESC => 0x29,
        KeyCode::KEY_BACKSPACE => 0x2A,
        KeyCode::KEY_TAB => 0x2B,
        KeyCode::KEY_SPACE => 0x2C,

        // ----- 符号 -----
        KeyCode::KEY_MINUS => 0x2D,
        KeyCode::KEY_EQUAL => 0x2E,
        KeyCode::KEY_LEFTBRACE => 0x2F,
        KeyCode::KEY_RIGHTBRACE => 0x30,
        KeyCode::KEY_BACKSLASH => 0x31,
        KeyCode::KEY_SEMICOLON => 0x33,
        KeyCode::KEY_APOSTROPHE => 0x34,
        KeyCode::KEY_GRAVE => 0x35,
        KeyCode::KEY_COMMA => 0x36,
        KeyCode::KEY_DOT => 0x37,
        KeyCode::KEY_SLASH => 0x38,
        KeyCode::KEY_CAPSLOCK => 0x39,

        // ----- 功能键 F1~F12 -----
        KeyCode::KEY_F1 => 0x3A,
        KeyCode::KEY_F2 => 0x3B,
        KeyCode::KEY_F3 => 0x3C,
        KeyCode::KEY_F4 => 0x3D,
        KeyCode::KEY_F5 => 0x3E,
        KeyCode::KEY_F6 => 0x3F,
        KeyCode::KEY_F7 => 0x40,
        KeyCode::KEY_F8 => 0x41,
        KeyCode::KEY_F9 => 0x42,
        KeyCode::KEY_F10 => 0x43,
        KeyCode::KEY_F11 => 0x44,
        KeyCode::KEY_F12 => 0x45,

        // ----- 功能区 -----
        KeyCode::KEY_SYSRQ | KeyCode::KEY_PRINT => 0x46, // PrintScreen
        KeyCode::KEY_SCROLLLOCK => 0x47,
        KeyCode::KEY_PAUSE => 0x48,
        KeyCode::KEY_INSERT => 0x49,
        KeyCode::KEY_HOME => 0x4A,
        KeyCode::KEY_PAGEUP => 0x4B,
        KeyCode::KEY_DELETE => 0x4C,
        KeyCode::KEY_END => 0x4D,
        KeyCode::KEY_PAGEDOWN => 0x4E,

        // ----- 方向键 -----
        KeyCode::KEY_RIGHT => 0x4F,
        KeyCode::KEY_LEFT => 0x50,
        KeyCode::KEY_DOWN => 0x51,
        KeyCode::KEY_UP => 0x52,

        // ----- 小键盘 -----
        KeyCode::KEY_NUMLOCK => 0x53,
        KeyCode::KEY_KPSLASH => 0x54,
        KeyCode::KEY_KPASTERISK => 0x55,
        KeyCode::KEY_KPMINUS => 0x56,
        KeyCode::KEY_KPPLUS => 0x57,
        KeyCode::KEY_KPENTER => 0x58,
        KeyCode::KEY_KP1 => 0x59,
        KeyCode::KEY_KP2 => 0x5A,
        KeyCode::KEY_KP3 => 0x5B,
        KeyCode::KEY_KP4 => 0x5C,
        KeyCode::KEY_KP5 => 0x5D,
        KeyCode::KEY_KP6 => 0x5E,
        KeyCode::KEY_KP7 => 0x5F,
        KeyCode::KEY_KP8 => 0x60,
        KeyCode::KEY_KP9 => 0x61,
        KeyCode::KEY_KP0 => 0x62,
        KeyCode::KEY_KPDOT => 0x63,
        KeyCode::KEY_102ND => 0x64, // 非美式键盘的 \| 键

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn test_input_manager() {
        info!("Starting InputManager test. Please provide keyboard/mouse input...");
        let mut manager = InputManager::new();

        while let Some(report) = manager.next_event().await {
            debug!("Input report: {:?}", report);
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_set_all_leds() {
        info!("Starting LED control test. Please observe keyboard LEDs...");
        let mut manager = InputManager::new();
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

        let led_handle = manager.led_handle.take().unwrap();
        for _ in 0..100 {
            led_handle.set_leds(&led_state_1).await;
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            led_handle.set_leds(&led_state_2).await;
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
        info!("Sent LED state to all keyboards.");
    }
}
