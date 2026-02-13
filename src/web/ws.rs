use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};

use futures::SinkExt;
use log::{error, info};

use std::sync::Arc;
use tokio::sync::Mutex;

// WebSocket 连接状态
pub struct WsState {
    active_socket: Mutex<Option<Arc<Mutex<WebSocket>>>>,
}

impl WsState {
    pub fn new() -> Self {
        Self {
            active_socket: Mutex::new(None),
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
                        handle_binary_message(&data);
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

fn handle_binary_message(data: &[u8]) {
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
                info!("鼠标移动: x={}, y={}", x, y);
            }
        }
        0x02 => {
            // 鼠标点击
            if data.len() >= 3 {
                let button = data[1];
                let state = data[2];
                info!("鼠标点击: button={}, state={}", button, state);
            }
        }
        0x03 => {
            // 滚轮
            if data.len() >= 5 {
                let x = i16::from_le_bytes([data[1], data[2]]);
                let y = i16::from_le_bytes([data[3], data[4]]);
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
