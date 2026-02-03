use crate::input::{InputManager, InputReport, LedHandle};
use crate::output::bluetooth_ble::{
    BluetoothBleMouseHidDevice, build_ble_hid_device, run_ble_server,
};
use crate::output::usb::{UsbMouseHidDevice, build_usb_hid_device};
use crate::output::{HidLedReader, HidReportSender, LedState, NoLedDevice};
use log::{debug, info, warn};

use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, watch};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputMode {
    Usb,
    Ble,
}

pub struct Core {
    input_manager: Arc<Mutex<InputManager>>,
    led_handle: Arc<Mutex<LedHandle>>,
    loop_cancellation_token: tokio_util::sync::CancellationToken,
    mode: Arc<RwLock<OutputMode>>,
    mode_tx: watch::Sender<OutputMode>,
    mode_rx: watch::Receiver<OutputMode>,
}

impl Core {
    pub fn new() -> Self {
        let mut manager = InputManager::new(500);
        let led_handle = manager.led_handle.take().unwrap();
        let (mode_tx, mode_rx) = watch::channel(OutputMode::Usb);

        Self {
            input_manager: Arc::new(Mutex::new(manager)),
            led_handle: Arc::new(Mutex::new(led_handle)),
            loop_cancellation_token: tokio_util::sync::CancellationToken::new(),
            mode: Arc::new(RwLock::new(OutputMode::Usb)),
            mode_tx,
            mode_rx,
        }
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        let (usb_kb, usb_kb_led, usb_mouse) = build_usb_hid_device().await?;
        let (ble_kb, ble_mouse, _session) = build_ble_hid_device().await?;
        let (_app_handle, _adv_handle) = run_ble_server(&ble_kb, &ble_mouse).await?;

        let usb_kb_sender: Arc<Mutex<Box<dyn HidReportSender>>> =
            Arc::new(Mutex::new(Box::new(usb_kb)));
        let usb_mouse_sender: Arc<Mutex<Box<dyn HidReportSender>>> =
            Arc::new(Mutex::new(Box::new(usb_mouse)));

        let ble_kb_sender: Arc<Mutex<Box<dyn HidReportSender>>> =
            Arc::new(Mutex::new(Box::new(ble_kb)));
        let ble_mouse_sender: Arc<Mutex<Box<dyn HidReportSender>>> =
            Arc::new(Mutex::new(Box::new(ble_mouse)));

        let usb_led_reader: Arc<Mutex<Box<dyn HidLedReader>>> =
            Arc::new(Mutex::new(Box::new(usb_kb_led)));
        let ble_led_reader: Arc<Mutex<Box<dyn HidLedReader>>> =
            Arc::new(Mutex::new(Box::new(NoLedDevice)));

        let main = self.main_loop(
            usb_kb_sender.clone(),
            usb_mouse_sender.clone(),
            ble_kb_sender.clone(),
            ble_mouse_sender.clone(),
        );

        let led = self.led_loop(usb_led_reader, ble_led_reader, self.mode_rx.clone());

        tokio::select! {
            _ = main => {},
            _ = led => {},
        }

        Ok(())
    }

    async fn main_loop(
        &self,
        usb_keyboard: Arc<Mutex<Box<dyn HidReportSender>>>,
        usb_mouse: Arc<Mutex<Box<dyn HidReportSender>>>,
        ble_keyboard: Arc<Mutex<Box<dyn HidReportSender>>>,
        ble_mouse: Arc<Mutex<Box<dyn HidReportSender>>>,
    ) {
        let cancellation_token = self.loop_cancellation_token.clone();
        let input_manager = Arc::clone(&self.input_manager);
        let mut switch_latched = false;

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    info!("主循环退出");
                    break;
                }
                event = async {
                    let mut mgr = input_manager.lock().await;
                    mgr.next_event().await
                } => {
                    if let Some(event) = event {
                        if self.should_toggle(&event, &mut switch_latched) {
                            self.toggle_output().await;
                            self.release_all(&usb_keyboard, &usb_mouse, &ble_keyboard, &ble_mouse).await;
                            let mode = *self.mode.read().await;
                            {
                                let mgr = input_manager.lock().await;
                                match mode {
                                    OutputMode::Usb => mgr.set_mouse_rate(500),
                                    OutputMode::Ble => mgr.set_mouse_rate(125),
                                }
                            }
                            continue;
                        }
                        let mode = *self.mode.read().await;
                        let result = match (&event, mode) {
                            (InputReport::Keyboard { .. }, OutputMode::Usb) => {
                                usb_keyboard.lock().await.send_report(event).await
                            }
                            (InputReport::Mouse { .. }, OutputMode::Usb) => {
                                usb_mouse.lock().await.send_report(event).await
                            }
                            (InputReport::Keyboard { .. }, OutputMode::Ble) => {
                                ble_keyboard.lock().await.send_report(event).await
                            }
                            (InputReport::Mouse { .. }, OutputMode::Ble) => {
                                ble_mouse.lock().await.send_report(event).await
                            }
                        };

                        if result.is_err() {
                            info!("发送 HID 报告出错，退出主循环");
                            break;
                        }
                    }
                }
            }
        }
    }

    async fn led_loop(
        &self,
        usb_led_reader: Arc<Mutex<Box<dyn HidLedReader>>>,
        ble_led_reader: Arc<Mutex<Box<dyn HidLedReader>>>,
        mut mode_rx: watch::Receiver<OutputMode>,
    ) {
        let cancellation_token = self.loop_cancellation_token.clone();
        let led_handle = Arc::clone(&self.led_handle);
        let mut current_led_state: LedState = LedState::default();

        loop {
            let mode = *mode_rx.borrow();
            let read_future = async {
                match mode {
                    OutputMode::Usb => usb_led_reader.lock().await.get_led_state().await,
                    OutputMode::Ble => ble_led_reader.lock().await.get_led_state().await,
                }
            };

            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    info!("LED 任务退出");
                    break;
                }
                _ = mode_rx.changed() => {
                    current_led_state = LedState::default();
                    continue;
                }
                result = read_future => {
                    match result {
                        Ok(Some(state)) => {
                            if current_led_state != state {
                                let handle = led_handle.lock().await;
                                handle.set_leds(&state).await;
                                current_led_state = state;
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            warn!("读取 LED 状态时出错: {:?}", e);
                            break;
                        }
                    }
                }
            }
        }
    }

    async fn toggle_output(&self) {
        let mut mode = self.mode.write().await;
        *mode = match *mode {
            OutputMode::Usb => OutputMode::Ble,
            OutputMode::Ble => OutputMode::Usb,
        };
        let _ = self.mode_tx.send(*mode);
        info!("当前输出切换为: {:?}", *mode);
    }

    fn should_toggle(&self, event: &InputReport, switch_latched: &mut bool) -> bool {
        match event {
            InputReport::Keyboard { modifiers, keys } => {
                let hit = is_switch_combo(*modifiers, keys);
                if hit && !*switch_latched {
                    *switch_latched = true;
                    return true;
                }
                if !hit && *switch_latched {
                    *switch_latched = false;
                }
                false
            }
            _ => false,
        }
    }

    async fn release_all(
        &self,
        usb_keyboard: &Arc<Mutex<Box<dyn HidReportSender>>>,
        usb_mouse: &Arc<Mutex<Box<dyn HidReportSender>>>,
        ble_keyboard: &Arc<Mutex<Box<dyn HidReportSender>>>,
        ble_mouse: &Arc<Mutex<Box<dyn HidReportSender>>>,
    ) {
        let empty_kb = InputReport::Keyboard {
            modifiers: 0,
            keys: vec![],
        };
        let empty_mouse = InputReport::Mouse {
            buttons: 0,
            x: 0,
            y: 0,
            wheel: 0,
        };

        let _ = usb_keyboard
            .lock()
            .await
            .send_report(empty_kb.clone())
            .await;
        let _ = usb_mouse
            .lock()
            .await
            .send_report(empty_mouse.clone())
            .await;
        let _ = ble_keyboard.lock().await.send_report(empty_kb).await;
        let _ = ble_mouse.lock().await.send_report(empty_mouse).await;
    }
}

// 默认切换组合键：Ctrl + Alt + F12
fn is_switch_combo(modifiers: u8, keys: &Vec<u8>) -> bool {
    let ctrl = modifiers & 0x01 != 0 || modifiers & 0x10 != 0;
    let alt = modifiers & 0x04 != 0 || modifiers & 0x40 != 0;
    let f12 = keys.contains(&0x45);
    ctrl && alt && f12
}
