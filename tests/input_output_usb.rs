use bridge_hid::input::{self, InputManager};
use bridge_hid::logging::init;
use bridge_hid::output::usb::build_usb_hid_device;
use bridge_hid::output::{HidLedReader, HidReportSender, LedState};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn test_usb_input_output() {
    init();
    println!("Starting USB input-output test...");
    let mut manager = InputManager::new(500);
    let mut led_handle = manager.led_handle.take().unwrap();

    loop {
        let (mut kb_hid_device, mut kb_hid_device_clone, mut mouse_hid_device) =
            build_usb_hid_device().await.expect("创建 USB HID 设备失败");

        let mouse_rate_controller = manager.mouse_rate_controller.clone();

        // std::thread::sleep(std::time::Duration::from_secs(2));
        let (manager_tx, manager_rx) = oneshot::channel();
        let (led_tx, led_rx) = oneshot::channel();

        let cancel_token_main = CancellationToken::new();
        let cancel_token_led = cancel_token_main.clone();
        let cancel_token_clone = cancel_token_main.clone();
        manager.clear_events().await;

        let main_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel_token_main.cancelled() => {
                        eprintln!("主任务收到取消信号");
                        break;
                    }
                    event = manager.next_event() => {
                        if let Some(event) = event {
                            let result = match event {
                                input::InputReport::Keyboard { .. } => {
                                    kb_hid_device.send_report(event).await
                                }
                                input::InputReport::Mouse { .. } => {
                                    mouse_hid_device.send_report(event).await
                                }
                            };
                            if result.is_err() {
                                eprintln!("发送事件失败，重新连接...");
                                break;
                            }
                        }
                    }
                }
            }
            let _ = manager_tx.send(manager);
        });

        let led_task = tokio::spawn(async move {
            let mut current_led_state: LedState = LedState::default();
            loop {
                tokio::select! {
                    _ = cancel_token_led.cancelled() => {
                        eprintln!("LED 任务收到取消信号");
                        break;
                    }
                    result = kb_hid_device_clone.get_led_state() => {
                        match result {
                            Ok(Some(state)) => {
                                if current_led_state != state {
                                    println!("LED State: {:?}", state);
                                    led_handle.set_leds(&state).await;
                                    current_led_state = state;
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                eprintln!("获取 LED 状态失败: {:?}，重新连接...", e);
                                break;
                            }
                        }
                    }
                }
            }
            let _ = led_tx.send(led_handle);
        });

        // 等待任意一个任务完成
        tokio::select! {
            _ = main_handle => {
                cancel_token_clone.cancel(); // 通知 LED 任务退出
            },
            _ = led_task => {
                cancel_token_clone.cancel(); // 通知主任务退出
            },
        }

        manager = manager_rx.await.expect("无法取回 manager");
        led_handle = led_rx.await.expect("无法取回 led_handle");
    }
}
