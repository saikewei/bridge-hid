use crate::web::ws;
use axum::{Router, routing::get};
use std::sync::Arc;
use tower_http::services::ServeDir;

pub async fn build_router() -> Router {
    let ws_state = Arc::new(ws::WsState::new().await);

    Router::new()
        .route("/ws", get(ws::ws_handler))
        .with_state(ws_state)
        .fallback_service(ServeDir::new("static"))
}
