use bridge_hid::input::{self, InputManager};
use bridge_hid::output::usb::{self, build_usb_hid_device};
use bridge_hid::output::{self, HidBackend};
use evdev::InputEvent;

use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn test_usb_input_output() {
    println!("Starting USB input-output test...");
    let mut manager = InputManager::new(None);

    let (mut kb_hid_device, mut mouse_hid_device) =
        build_usb_hid_device().expect("创建 USB HID 设备失败");

    std::thread::sleep(std::time::Duration::from_secs(2));

    let _ = tokio::spawn(async move {
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
    })
    .await;
}
