use crate::web::ws;
use axum::{Router, routing::get};
use tower_http::services::ServeDir;

pub fn build_router() -> Router {
    let router = Router::new()
        // .route("/api/hello", get(hello_handler))
        .route("/ws", get(ws::ws_handler))
        .fallback_service(ServeDir::new("static"));

    return router;
}
