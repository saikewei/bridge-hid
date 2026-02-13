use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
};

pub async fn ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    // 完成握手并处理连接
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    // 循环接收消息
    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Binary(msg) => {
                // 处理二进制消息
                println!("Received binary message: {:?}", msg);
            }
            _ => (),
        }
    }
}
