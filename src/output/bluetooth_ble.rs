use anyhow::{Result, anyhow};
use async_trait::async_trait;
use bluer::adv::{Advertisement, AdvertisementHandle};
use bluer::agent::Agent;
use bluer::gatt::local::{
    Application, Characteristic, CharacteristicNotify, CharacteristicNotifyMethod,
    CharacteristicRead, CharacteristicWrite, CharacteristicWriteMethod, Descriptor, DescriptorRead,
    DescriptorWrite, Service,
};
use bluer::{Adapter, Uuid};
use futures::FutureExt;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

use super::{HidLedReader, HidReportSender, InputReport, LedState};

macro_rules! ble_uuid {
    ($short:expr) => {
        Uuid::from_u128((($short as u128) << 96) | 0x0000_0000_1000_8000_00805f9b34fb_u128)
    };
}

// BLE HID 相关 UUID
const HID_SERVICE_UUID: Uuid = ble_uuid!(0x1812);
const HID_REPORT_MAP_UUID: Uuid = ble_uuid!(0x2A4B);
const HID_REPORT_UUID: Uuid = ble_uuid!(0x2A4D);
const HID_INFORMATION_UUID: Uuid = ble_uuid!(0x2A4A);
const HID_CONTROL_POINT_UUID: Uuid = ble_uuid!(0x2A4C);
const PROTOCOL_MODE_UUID: Uuid = ble_uuid!(0x2A4E);

const BATTERY_SERVICE_UUID: Uuid = ble_uuid!(0x180F);
const BATTERY_LEVEL_UUID: Uuid = ble_uuid!(0x2A19);

const DEVICE_INFO_SERVICE_UUID: Uuid = ble_uuid!(0x180A);
const MANUFACTURER_NAME_UUID: Uuid = ble_uuid!(0x2A29);
const MODEL_NUMBER_UUID: Uuid = ble_uuid!(0x2A24);
const PNP_ID_UUID: Uuid = ble_uuid!(0x2A50);

const REPORT_REFERENCE_UUID: Uuid = ble_uuid!(0x2908);

// 使用和 Python 版本完全相同的 HID Report Descriptor
// 带有 Report ID = 1
const HID_REPORT_MAP: &[u8] = &[
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x06, // Usage (Keyboard)
    0xA1, 0x01, // Collection (Application)
    0x85, 0x01, //   Report ID (1)  <-- 重要！
    0x05, 0x07, //   Usage Page (Key Codes)
    0x19, 0xE0, //   Usage Minimum (224)
    0x29, 0xE7, //   Usage Maximum (231)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x01, //   Logical Maximum (1)
    0x75, 0x01, //   Report Size (1)
    0x95, 0x08, //   Report Count (8)
    0x81, 0x02, //   Input (Data, Variable, Absolute) - Modifier byte
    0x75, 0x08, //   Report Size (8)
    0x95, 0x01, //   Report Count (1)
    0x81, 0x01, //   Input (Constant) - Reserved byte
    0x05, 0x08, //   Usage Page (LEDs)
    0x75, 0x01, //   Report Size (1)
    0x95, 0x05, //   Report Count (5)
    0x19, 0x01, //   Usage Minimum (1)
    0x29, 0x05, //   Usage Maximum (5)
    0x91, 0x02, //   Output (Data, Variable, Absolute) - LED report
    0x75, 0x03, //   Report Size (3)
    0x95, 0x01, //   Report Count (1)
    0x91, 0x01, //   Output (Constant) - Padding
    0x05, 0x07, //   Usage Page (Key Codes)
    0x19, 0x00, //   Usage Minimum (0)
    0x2A, 0xFF, 0x00, // Usage Maximum (255)
    0x15, 0x00, //   Logical Minimum (0)
    0x26, 0xFF, 0x00, // Logical Maximum (255)
    0x75, 0x08, //   Report Size (8)
    0x95, 0x06, //   Report Count (6)
    0x81, 0x00, //   Input (Data, Array) - Key array
    0xC0, // End Collection
    // ----- Mouse (Report ID 2) -----
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x02, // Usage (Mouse)
    0xA1, 0x01, // Collection (Application)
    0x85, 0x02, //   Report ID (2)
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
    0x81, 0x06, //     Input (Data, Variable, Relative)
    0xC0, //   End Collection
    0xC0, // End Collection
];

// HID Information: bcdHID=1.11, bCountryCode=0, Flags=0x02 (normally connectable)
const HID_INFORMATION: &[u8] = &[0x01, 0x11, 0x00, 0x02];

type ReportNotifier = mpsc::Sender<Vec<u8>>;

pub struct BluetoothBleKeyboardHidDevice {
    adapter: Arc<Adapter>,
    keyboard_notifier: Arc<Mutex<Option<ReportNotifier>>>,
    #[allow(dead_code)]
    session: bluer::Session,
    #[allow(dead_code)]
    _agent_handle: Arc<bluer::agent::AgentHandle>,
}

pub struct BluetoothBleMouseHidDevice {
    #[allow(dead_code)]
    adapter: Arc<Adapter>,
    #[allow(dead_code)]
    mouse_notifier: Arc<Mutex<Option<ReportNotifier>>>,
    #[allow(dead_code)]
    session: bluer::Session,
    #[allow(dead_code)]
    _agent_handle: Arc<bluer::agent::AgentHandle>,
}

struct BleHidState {
    keyboard_notifier: Arc<Mutex<Option<ReportNotifier>>>,
    mouse_notifier: Arc<Mutex<Option<ReportNotifier>>>,
}

pub async fn build_ble_hid_device() -> Result<(
    BluetoothBleKeyboardHidDevice,
    BluetoothBleMouseHidDevice,
    bluer::Session,
)> {
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;

    // 配置适配器
    adapter.set_powered(true).await?;
    adapter.set_alias("BLE Keyboard".to_string()).await?;
    adapter.set_discoverable(true).await?;
    adapter.set_discoverable_timeout(0).await?;
    adapter.set_pairable(true).await?;
    adapter.set_pairable_timeout(0).await?;

    log::info!("BLE 适配器已配置: {}", adapter.name());
    log::info!("适配器地址: {}", adapter.address().await?);

    // Agent 配置 - 使用 KeyboardOnly capability（和 Python 版本一致）
    let agent = Agent {
        request_default: true,
        request_passkey: Some(Box::new(|req| {
            Box::pin(async move {
                log::info!("请求 Passkey，设备: {}", req.device);
                // 可以在这里实现真正的 passkey 输入
                Ok(123456)
            })
        })),
        display_passkey: Some(Box::new(|req| {
            Box::pin(async move {
                log::info!("显示 Passkey: {} (已输入: {})", req.passkey, req.entered);
                Ok(())
            })
        })),
        request_confirmation: Some(Box::new(|req| {
            Box::pin(async move {
                log::info!("确认配对请求，passkey: {}", req.passkey);
                Ok(())
            })
        })),
        authorize_service: Some(Box::new(|req| {
            Box::pin(async move {
                log::info!("授权服务: 设备 {} 访问 {}", req.device, req.service);
                Ok(())
            })
        })),
        request_authorization: Some(Box::new(|req| {
            Box::pin(async move {
                log::info!("授权请求: {}", req.device);
                Ok(())
            })
        })),
        ..Default::default()
    };

    let agent_handle = session.register_agent(agent).await?;
    log::info!("Agent 已注册");

    let adapter = Arc::new(adapter);
    let keyboard_notifier = Arc::new(Mutex::new(None));
    let mouse_notifier = Arc::new(Mutex::new(None));
    let shared_handle = Arc::new(agent_handle);

    let keyboard = BluetoothBleKeyboardHidDevice {
        adapter: Arc::clone(&adapter),
        keyboard_notifier: Arc::clone(&keyboard_notifier),
        session: session.clone(),
        _agent_handle: Arc::clone(&shared_handle),
    };

    let mouse = BluetoothBleMouseHidDevice {
        adapter: Arc::clone(&adapter),
        mouse_notifier: Arc::clone(&mouse_notifier),
        session: session.clone(),
        _agent_handle: Arc::clone(&shared_handle),
    };

    Ok((keyboard, mouse, session))
}

pub async fn run_ble_server(
    keyboard: &BluetoothBleKeyboardHidDevice,
    mouse: &BluetoothBleMouseHidDevice,
) -> Result<(bluer::gatt::local::ApplicationHandle, AdvertisementHandle)> {
    let adapter = &keyboard.adapter;

    let state = Arc::new(BleHidState {
        keyboard_notifier: Arc::clone(&keyboard.keyboard_notifier),
        mouse_notifier: Arc::clone(&mouse.mouse_notifier),
    });

    let app = build_gatt_application(state).await?;
    let app_handle = adapter.serve_gatt_application(app).await?;
    log::info!("GATT 应用已注册");

    // 广播配置
    let adv = Advertisement {
        advertisement_type: bluer::adv::Type::Peripheral,
        service_uuids: vec![HID_SERVICE_UUID, BATTERY_SERVICE_UUID]
            .into_iter()
            .collect(),
        local_name: Some("BLE Keyboard".to_string()),
        appearance: Some(0x03C2), // Keyboard+Mouse
        discoverable: Some(true),
        ..Default::default()
    };

    let adv_handle = adapter.advertise(adv).await?;
    log::info!("BLE 广播已启动");

    Ok((app_handle, adv_handle))
}

async fn build_gatt_application(state: Arc<BleHidState>) -> Result<Application> {
    let keyboard_notifier = Arc::clone(&state.keyboard_notifier);
    let mouse_notifier = Arc::clone(&state.mouse_notifier);

    // HID Service
    let hid_service = Service {
        uuid: HID_SERVICE_UUID,
        primary: true,
        characteristics: vec![
            // Protocol Mode
            Characteristic {
                uuid: PROTOCOL_MODE_UUID,
                read: Some(CharacteristicRead {
                    read: true,
                    fun: Box::new(|_req| {
                        async move {
                            log::debug!("读取 Protocol Mode");
                            Ok(vec![0x01]) // Report Protocol
                        }
                        .boxed()
                    }),
                    ..Default::default()
                }),
                write: Some(CharacteristicWrite {
                    write_without_response: true,
                    method: CharacteristicWriteMethod::Fun(Box::new(|new_value, _req| {
                        async move {
                            log::info!("Protocol Mode 写入: {:?}", new_value);
                            Ok(())
                        }
                        .boxed()
                    })),
                    ..Default::default()
                }),
                ..Default::default()
            },
            // HID Information - 使用 secure read
            Characteristic {
                uuid: HID_INFORMATION_UUID,
                read: Some(CharacteristicRead {
                    read: true,
                    encrypt_read: true, // 加密读取
                    fun: Box::new(|_req| {
                        async move {
                            log::debug!("读取 HID Information");
                            Ok(HID_INFORMATION.to_vec())
                        }
                        .boxed()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            // Report Map
            Characteristic {
                uuid: HID_REPORT_MAP_UUID,
                read: Some(CharacteristicRead {
                    read: true,
                    fun: Box::new(|_req| {
                        async move {
                            log::info!("读取 Report Map ({} bytes)", HID_REPORT_MAP.len());
                            Ok(HID_REPORT_MAP.to_vec())
                        }
                        .boxed()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            // HID Control Point
            Characteristic {
                uuid: HID_CONTROL_POINT_UUID,
                write: Some(CharacteristicWrite {
                    write_without_response: true,
                    method: CharacteristicWriteMethod::Fun(Box::new(|new_value, _req| {
                        async move {
                            log::info!("HID Control Point 写入: {:?}", new_value);
                            Ok(())
                        }
                        .boxed()
                    })),
                    ..Default::default()
                }),
                ..Default::default()
            },
            // Report Characteristic - 键盘输入报告
            Characteristic {
                uuid: HID_REPORT_UUID,
                read: Some(CharacteristicRead {
                    read: true,
                    encrypt_read: true,
                    fun: Box::new(|_req| {
                        async move {
                            log::debug!("读取 Report");
                            // 不包含 Report ID: [modifier, reserved, 6 keys]
                            Ok(vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00])
                        }
                        .boxed()
                    }),
                    ..Default::default()
                }),

                notify: Some(CharacteristicNotify {
                    notify: true,
                    method: CharacteristicNotifyMethod::Fun(Box::new(move |mut notifier| {
                        let keyboard_notifier = Arc::clone(&keyboard_notifier);
                        async move {
                            let (tx, mut rx) = mpsc::channel::<Vec<u8>>(16);
                            {
                                let mut guard = keyboard_notifier.lock().await;
                                *guard = Some(tx);
                            }
                            log::info!("键盘 Report 通知已启用");

                            while let Some(report) = rx.recv().await {
                                log::debug!("发送键盘报告: {:02X?}", report);
                                if let Err(e) = notifier.notify(report).await {
                                    log::error!("通知发送失败: {}", e);
                                    break;
                                }
                            }
                            log::info!("键盘 Report 通知已停止");
                        }
                        .boxed()
                    })),
                    ..Default::default()
                }),
                descriptors: vec![
                    // Report Reference Descriptor
                    Descriptor {
                        uuid: REPORT_REFERENCE_UUID,
                        read: Some(DescriptorRead {
                            read: true,
                            fun: Box::new(|_req| {
                                async move {
                                    log::debug!("读取 Report Reference");
                                    // [Report ID=1, Type=Input(0x01)]
                                    // 必须和 Report Descriptor 中的 Report ID 一致！
                                    Ok(vec![0x01, 0x01])
                                }
                                .boxed()
                            }),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            // Report Characteristic - 鼠标输入报告 (Report ID 2)
            Characteristic {
                uuid: HID_REPORT_UUID,
                // 鼠标 Report 读取
                read: Some(CharacteristicRead {
                    read: true,
                    encrypt_read: true,
                    fun: Box::new(|_req| {
                        async move {
                            log::debug!("读取 Mouse Report");
                            // 不包含 Report ID: [buttons, x, y, wheel]
                            Ok(vec![0x00, 0x00, 0x00, 0x00])
                        }
                        .boxed()
                    }),
                    ..Default::default()
                }),
                notify: Some(CharacteristicNotify {
                    notify: true,
                    method: CharacteristicNotifyMethod::Fun(Box::new(move |mut notifier| {
                        let mouse_notifier = Arc::clone(&mouse_notifier);
                        async move {
                            let (tx, mut rx) = mpsc::channel::<Vec<u8>>(16);
                            {
                                let mut guard = mouse_notifier.lock().await;
                                *guard = Some(tx);
                            }
                            log::info!("鼠标 Report 通知已启用");

                            while let Some(report) = rx.recv().await {
                                log::debug!("发送鼠标报告: {:02X?}", report);
                                if let Err(e) = notifier.notify(report).await {
                                    log::error!("通知发送失败: {}", e);
                                    break;
                                }
                            }
                            log::info!("鼠标 Report 通知已停止");
                        }
                        .boxed()
                    })),
                    ..Default::default()
                }),
                descriptors: vec![Descriptor {
                    uuid: REPORT_REFERENCE_UUID,
                    read: Some(DescriptorRead {
                        read: true,
                        fun: Box::new(|_req| {
                            async move {
                                log::debug!("读取 Mouse Report Reference");
                                // [Report ID=2, Type=Input(0x01)]
                                Ok(vec![0x02, 0x01])
                            }
                            .boxed()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    // Battery Service
    let battery_service = Service {
        uuid: BATTERY_SERVICE_UUID,
        primary: true,
        characteristics: vec![Characteristic {
            uuid: BATTERY_LEVEL_UUID,
            read: Some(CharacteristicRead {
                read: true,
                fun: Box::new(|_req| {
                    async move {
                        log::debug!("读取电池电量");
                        Ok(vec![100u8])
                    }
                    .boxed()
                }),
                ..Default::default()
            }),
            notify: Some(CharacteristicNotify {
                notify: true,
                method: CharacteristicNotifyMethod::Fun(Box::new(|_notifier| {
                    async move {
                        log::info!("电池通知已启用");
                    }
                    .boxed()
                })),
                ..Default::default()
            }),
            ..Default::default()
        }],
        ..Default::default()
    };

    // Device Information Service
    let device_info_service = Service {
        uuid: DEVICE_INFO_SERVICE_UUID,
        primary: true,
        characteristics: vec![
            Characteristic {
                uuid: MANUFACTURER_NAME_UUID,
                read: Some(CharacteristicRead {
                    read: true,
                    fun: Box::new(|_req| async move { Ok(b"artyomsoft".to_vec()) }.boxed()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Characteristic {
                uuid: MODEL_NUMBER_UUID,
                read: Some(CharacteristicRead {
                    read: true,
                    fun: Box::new(|_req| async move { Ok(b"BLE Keyboard".to_vec()) }.boxed()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Characteristic {
                uuid: PNP_ID_UUID,
                read: Some(CharacteristicRead {
                    read: true,
                    fun: Box::new(|_req| {
                        // PnP ID 和 Python 版本一致
                        // 02 C4 10 01 00 01 00
                        // VID Source=0x02, VID=0x10C4, PID=0x0001, Version=0x0001
                        async move { Ok(vec![0x02, 0xC4, 0x10, 0x01, 0x00, 0x01, 0x00]) }.boxed()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    Ok(Application {
        services: vec![hid_service, device_info_service, battery_service],
        ..Default::default()
    })
}

#[async_trait]
impl HidReportSender for BluetoothBleKeyboardHidDevice {
    async fn send_report(&mut self, report: InputReport) -> Result<()> {
        if let InputReport::Keyboard { modifiers, keys } = report {
            let guard = self.keyboard_notifier.lock().await;
            if let Some(ref tx) = *guard {
                // BLE HID 通知时不包含 Report ID！
                // Report ID 通过 Report Reference Descriptor 标识
                // 只发送: [modifier, reserved, 6 keys] = 8 字节
                let mut hid_report = Vec::with_capacity(8);
                hid_report.push(modifiers);
                hid_report.push(0x00); // reserved
                for i in 0..6 {
                    hid_report.push(*keys.get(i).unwrap_or(&0x00));
                }

                log::info!("发送键盘报告: {:02X?}", hid_report);
                tx.send(hid_report)
                    .await
                    .map_err(|e| anyhow!("发送键盘报告失败: {}", e))?;
            } else {
                log::warn!("键盘通知器未就绪");
            }
        }
        Ok(())
    }
}

#[async_trait]
impl HidReportSender for BluetoothBleMouseHidDevice {
    async fn send_report(&mut self, report: InputReport) -> Result<()> {
        if let InputReport::Mouse {
            buttons,
            x,
            y,
            wheel,
        } = report
        {
            let guard = self.mouse_notifier.lock().await;
            if let Some(ref tx) = *guard {
                let clamp_i8 = |v: i16| -> i8 {
                    if v > 127 {
                        127
                    } else if v < -127 {
                        -127
                    } else {
                        v as i8
                    }
                };
                let x = clamp_i8(x) as u8;
                let y = clamp_i8(y) as u8;
                let wheel = (wheel as i8) as u8;

                // BLE HID 通知时不包含 Report ID！
                // 只发送: [buttons, x, y, wheel] = 4 字节
                let hid_report = vec![buttons, x, y, wheel];
                log::info!("发送鼠标报告: {:02X?}", hid_report);
                tx.send(hid_report)
                    .await
                    .map_err(|e| anyhow!("发送鼠标报告失败: {}", e))?;
            } else {
                log::warn!("鼠标通知器未就绪");
            }
        }
        Ok(())
    }
}

#[async_trait]
impl HidLedReader for BluetoothBleKeyboardHidDevice {
    async fn get_led_state(&mut self) -> Result<Option<LedState>> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    #[ignore]
    async fn test_ble_hid_connection() -> Result<()> {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

        let (mut keyboard, mouse, _session) = build_ble_hid_device().await?;
        let (_app_handle, _adv_handle) = run_ble_server(&keyboard, &mouse).await?;

        println!("--------------------------------------------------");
        println!("BLE HID 测试已启动！");
        println!("请在 iPad 蓝牙设置中搜索并连接 'BLE Keyboard'");
        println!("--------------------------------------------------");

        for i in 0..120 {
            tokio::time::sleep(Duration::from_secs(1)).await;

            let is_ready = keyboard.keyboard_notifier.lock().await.is_some();

            if is_ready {
                println!("连接成功！等待 2 秒后发送测试按键...");
                tokio::time::sleep(Duration::from_secs(2)).await;

                let held_key = 0x04; // A
                println!("按住键: 0x{:02X}（不松手）", held_key);

                // 只发送一次按下，不发送松开
                keyboard
                    .send_report(InputReport::Keyboard {
                        modifiers: 0x00,
                        keys: vec![held_key],
                    })
                    .await?;

                println!("已按住，等待 30 秒...");
                tokio::time::sleep(Duration::from_secs(30)).await;
                break;
            } else if i % 10 == 0 {
                println!("等待连接... ({}s)", i);
            }
        }

        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn test_ble_mouse_square_motion() -> Result<()> {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

        let (_keyboard, mut mouse, _session) = build_ble_hid_device().await?;
        let (_app_handle, _adv_handle) = run_ble_server(&_keyboard, &mouse).await?;

        println!("--------------------------------------------------");
        println!("BLE 鼠标测试已启动！");
        println!("请在 iPad 蓝牙设置中搜索并连接 'BLE Keyboard'");
        println!("连接后将按住左键，从左上角画方形轨迹");
        println!("--------------------------------------------------");

        for i in 0..120 {
            tokio::time::sleep(Duration::from_secs(1)).await;

            let is_ready = mouse.mouse_notifier.lock().await.is_some();

            if is_ready {
                println!("鼠标连接成功！等待 2 秒后开始移动...");
                tokio::time::sleep(Duration::from_secs(2)).await;

                let step: i16 = 10;
                let steps_per_side = 20;
                let delay = Duration::from_millis(20);
                let left_button = 0x01;

                async fn send(
                    mouse: &mut BluetoothBleMouseHidDevice,
                    buttons: u8,
                    dx: i16,
                    dy: i16,
                ) -> Result<()> {
                    mouse
                        .send_report(InputReport::Mouse {
                            buttons,
                            x: dx,
                            y: dy,
                            wheel: 0,
                        })
                        .await
                }

                // 右
                for _ in 0..steps_per_side {
                    send(&mut mouse, left_button, step, 0).await?;
                    tokio::time::sleep(delay).await;
                }
                // 下
                for _ in 0..steps_per_side {
                    send(&mut mouse, left_button, 0, step).await?;
                    tokio::time::sleep(delay).await;
                }
                // 左
                for _ in 0..steps_per_side {
                    send(&mut mouse, left_button, -step, 0).await?;
                    tokio::time::sleep(delay).await;
                }
                // 上
                for _ in 0..steps_per_side {
                    send(&mut mouse, left_button, 0, -step).await?;
                    tokio::time::sleep(delay).await;
                }

                // 松开左键
                send(&mut mouse, 0x00, 0, 0).await?;

                println!("方形轨迹完成");
                break;
            } else if i % 10 == 0 {
                println!("等待鼠标连接... ({}s)", i);
            }
        }

        Ok(())
    }
}
