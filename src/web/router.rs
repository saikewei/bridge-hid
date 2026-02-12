use axum::Router;
use tower_http::services::ServeDir;

pub fn build_router() -> Router {
    let router = Router::new()
        // .route("/api/hello", get(hello_handler))
        .fallback_service(ServeDir::new("static"));

    return router;
}
