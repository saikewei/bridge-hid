use bridge_hid::input::{self, InputManager};
use bridge_hid::logging::init;
use bridge_hid::output::usb::{self, build_usb_hid_device};
use bridge_hid::output::{self, HidLedReader, HidReportSender, LedState};
use evdev::InputEvent;
use glob;

use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn test_usb_input_output() {
    init();
    println!("Starting USB input-output test...");
    let mut manager = InputManager::new();
    let mut led_handle = manager.led_handle.take().unwrap();

    let (mut kb_hid_device, mut kb_hid_device_clone, mut mouse_hid_device) =
        build_usb_hid_device().await.expect("创建 USB HID 设备失败");

    // std::thread::sleep(std::time::Duration::from_secs(2));

    let main_handle = tokio::spawn(async move {
        loop {
            if let Some(event) = manager.next_event().await {
                match event {
                    input::InputReport::Keyboard { .. } => {
                        kb_hid_device
                            .send_report(event)
                            .await
                            .expect("发送键盘事件失败");
                    }
                    input::InputReport::Mouse { .. } => {
                        mouse_hid_device
                            .send_report(event)
                            .await
                            .expect("发送鼠标事件失败");
                    }
                }
            }
        }
    });

    let led_handle = tokio::spawn(async move {
        let mut current_led_state: LedState = LedState::default();
        loop {
            let led_state = kb_hid_device_clone
                .get_led_state()
                .await
                .expect("获取 LED 状态失败");
            if let Some(state) = led_state {
                if current_led_state != state {
                    println!("LED State: {:?}", state);
                    led_handle.set_leds(&state).await;
                    current_led_state = state;
                }
            }
        }
    });

    tokio::select! {
        _ = main_handle => {},
        _ = led_handle => {},
    }
}
