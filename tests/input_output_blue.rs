use bridge_hid::input::{self, InputManager};
use bridge_hid::output::bluetooth_ble::{
    BluetoothBleKeyboardHidDevice, BluetoothBleMouseHidDevice, build_ble_hid_device, run_ble_server,
};
use bridge_hid::output::{self, HidLedReader, HidReportSender};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn test_blue_input_output() {
    println!("Starting blue input/output test...");
    let mut manager = InputManager::new();

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let (mut keyboard, mut mouse, _session) = build_ble_hid_device().await.unwrap();
    let (_app_handle, _adv_handle) = run_ble_server(&keyboard, &mouse).await.unwrap();

    tokio::spawn(async move {
        loop {
            if let Some(event) = manager.next_event().await {
                match event {
                    input::InputReport::Keyboard { .. } => {
                        keyboard.send_report(event).await.expect("发送键盘事件失败");
                    }
                    input::InputReport::Mouse { .. } => {
                        mouse.send_report(event).await.expect("发送鼠标事件失败");
                    }
                }
            }
        }
    })
    .await
    .unwrap();
}
