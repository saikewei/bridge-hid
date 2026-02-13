use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};

use futures::SinkExt;
use log::{error, info};
use usb_gadget::function::hid;

use std::sync::Arc;
use tokio::sync::Mutex;

use crate::output::{
    HidReportSender, UsbKeyboardHidDevice, UsbMouseHidDevice,
    usb::{UsbError, build_usb_hid_device},
};

use crate::input::{DeviceType, InputReport};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;

// WebSocket 连接状态
pub struct WsState {
    active_socket: Mutex<Option<Arc<Mutex<WebSocket>>>>,
    hid_guard: Arc<ReconnectGuard>,
}

impl WsState {
    pub async fn new() -> Self {
        let hid_guard = Arc::new(ReconnectGuard::new().await);
        Self {
            active_socket: Mutex::new(None),
            hid_guard,
        }
    }
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<WsState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<WsState>) {
    // 获取锁并替换旧连接
    let mut active = state.active_socket.lock().await;

    // 如果存在旧连接，关闭它
    if let Some(old_socket) = active.take() {
        info!("检测到旧连接，正在断开...");
        let mut old = old_socket.lock().await;
        let _ = old.close().await;
        drop(old);
        info!("旧连接已断开");
    }

    // 保存新连接
    let socket_arc = Arc::new(Mutex::new(socket));
    *active = Some(socket_arc.clone());
    drop(active); // 释放锁

    info!("新 WebSocket 连接已建立");

    // 处理消息
    loop {
        let mut sock = socket_arc.lock().await;
        match sock.recv().await {
            Some(Ok(msg)) => match msg {
                Message::Binary(data) => {
                    info!("收到二进制消息: {} bytes", data.len());
                    if data.len() > 0 {
                        handle_binary_message(&data, &state.hid_guard);
                    }
                }
                Message::Close(_) => {
                    info!("客户端关闭连接");
                    break;
                }
                _ => {}
            },
            Some(Err(e)) => {
                error!("WebSocket 错误: {}", e);
                break;
            }
            None => {
                info!("连接已关闭");
                break;
            }
        }
        drop(sock); // 释放锁
    }

    // 清理连接
    let mut active = state.active_socket.lock().await;
    *active = None;
    info!("WebSocket 连接已清理");
}

fn handle_binary_message(data: &[u8], hid_guard: &ReconnectGuard) {
    if data.is_empty() {
        return;
    }

    let msg_type = data[0];
    match msg_type {
        0x01 => {
            // 鼠标移动
            if data.len() >= 5 {
                let x = i16::from_le_bytes([data[1], data[2]]);
                let y = i16::from_le_bytes([data[3], data[4]]);
                let _ = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        hid_guard
                            .send_report(
                                DeviceType::Mouse,
                                InputReport::Mouse {
                                    buttons: 0, // 默认无按钮按下
                                    x,
                                    y,
                                    wheel: 0, // 默认无滚轮
                                },
                            )
                            .await
                    })
                });
                info!("鼠标移动: x={}, y={}", x, y);
            }
        }
        0x02 => {
            // 鼠标点击
            if data.len() >= 3 {
                let button = data[1];
                let state = data[2];
                let _ = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        hid_guard
                            .send_report(
                                DeviceType::Mouse,
                                InputReport::Mouse {
                                    buttons: button,
                                    x: 0,
                                    y: 0,
                                    wheel: 0,
                                },
                            )
                            .await
                    })
                });
                info!("鼠标点击: button={}, state={}", button, state);
            }
        }
        0x03 => {
            // 滚轮
            if data.len() >= 5 {
                let x = i16::from_le_bytes([data[1], data[2]]);
                let y = i16::from_le_bytes([data[3], data[4]]);
                let wheel = y.clamp(i8::MIN as i16, i8::MAX as i16) as i8;
                let _ = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        hid_guard
                            .send_report(
                                DeviceType::Mouse,
                                InputReport::Mouse {
                                    buttons: 0,
                                    x: 0,
                                    y: 0,
                                    wheel,
                                },
                            )
                            .await
                    })
                });
                info!("滚轮: x={}, y={}", x, y);
            }
        }
        0x04 => {
            // 键盘
            if data.len() >= 5 {
                let key_code = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
                if let Some(ch) = char::from_u32(key_code) {
                    info!("键盘输入: '{}'", ch);
                }
            }
        }
        _ => {
            info!("未知消息类型: 0x{:02X}", msg_type);
        }
    }
}

struct ReconnectGuard {
    keyboard: Arc<Mutex<Option<UsbKeyboardHidDevice>>>,
    mouse: Arc<Mutex<Option<UsbMouseHidDevice>>>,
    connected: Arc<AtomicBool>,
    reconnecting: Arc<AtomicBool>,
}

impl ReconnectGuard {
    async fn new() -> Self {
        let (keyboard, _, mouse) = build_usb_hid_device()
            .await
            .expect("请先连接电脑再启动程序！");

        Self {
            keyboard: Arc::new(Mutex::new(Some(keyboard))),
            mouse: Arc::new(Mutex::new(Some(mouse))),
            connected: Arc::new(AtomicBool::new(true)),
            reconnecting: Arc::new(AtomicBool::new(false)),
        }
    }

    async fn send_report(&self, device_type: DeviceType, report: InputReport) -> Result<()> {
        if !self.connected.load(Ordering::SeqCst) {
            return Ok(()); // 断连中，静默丢弃
        }

        let res = match device_type {
            DeviceType::Keyboard => {
                let mut guard = self.keyboard.lock().await;
                if let Some(ref mut kb) = *guard {
                    kb.send_report(report).await
                } else {
                    return Ok(());
                }
            }
            DeviceType::Mouse => {
                let mut guard = self.mouse.lock().await;
                if let Some(ref mut ms) = *guard {
                    ms.send_report(report).await
                } else {
                    return Ok(());
                }
            }
        };

        match res {
            Ok(_) => Ok(()),
            Err(e) => {
                if e.downcast_ref::<UsbError>().is_some() {
                    error!("USB 连接错误，尝试重连");
                    self.connected.store(false, Ordering::SeqCst);

                    if !self.reconnecting.swap(true, Ordering::SeqCst) {
                        let keyboard_clone = Arc::clone(&self.keyboard);
                        let mouse_clone = Arc::clone(&self.mouse);
                        let connected_clone = Arc::clone(&self.connected);
                        let reconnecting_clone = Arc::clone(&self.reconnecting);

                        tokio::spawn(async move {
                            info!("后台重连任务启动");
                            match Self::reconnect_devices(keyboard_clone, mouse_clone).await {
                                Ok(_) => {
                                    info!("USB 设备重连成功");
                                    connected_clone.store(true, Ordering::SeqCst);
                                }
                                Err(e) => {
                                    error!("USB 设备重连失败: {}", e);
                                }
                            }
                            reconnecting_clone.store(false, Ordering::SeqCst);
                        });
                    }
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }

    async fn reconnect_devices(
        keyboard: Arc<Mutex<Option<UsbKeyboardHidDevice>>>,
        mouse: Arc<Mutex<Option<UsbMouseHidDevice>>>,
    ) -> Result<()> {
        info!("正在尝试重建 USB HID 设备...");

        // ✅ 第一步：销毁旧设备，确保旧 RegGadget 完全释放
        {
            let mut kb = keyboard.lock().await;
            let mut ms = mouse.lock().await;

            // take() 会把 Option 变为 None，旧值被 drop
            let _old_kb = kb.take();
            let _old_ms = ms.take();

            // _old_kb, _old_ms 在作用域结束时 drop
            // 旧的 Arc<RegGadget> 引用计数归零 → 旧 gadget 被内核清理
        }

        // 等待内核完全释放旧设备节点
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // ✅ 第二步：创建全新的设备（此时没有同名旧 gadget 残留）
        let (new_keyboard, _, new_mouse) = build_usb_hid_device().await?;

        // ✅ 第三步：放入新设备
        *keyboard.lock().await = Some(new_keyboard);
        *mouse.lock().await = Some(new_mouse);

        info!("USB HID 设备已完全重建");
        Ok(())
    }
}
