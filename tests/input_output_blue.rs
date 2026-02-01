use bridge_hid::input::{self, InputManager};
use bridge_hid::output::bluetooth::{build_bluetooth_hid_device, run_server};
use bridge_hid::output::{self, HidLedReader, HidReportSender};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn test_blue_input_output() {
    println!("Starting blue input/output test...");
    let mut manager = InputManager::new();

    let (mut keyboard, mut mouse, session) = build_bluetooth_hid_device().await.unwrap();
    if let Err(e) = run_server(&keyboard, &session).await {
        eprintln!("服务器运行出错: {}", e);
    }

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
    .await;
}
