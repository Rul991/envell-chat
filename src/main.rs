use std::net::SocketAddr;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::services::{ServeDir, ServeFile};

#[derive(Deserialize, Serialize, Debug, Clone)]
struct Message {
    color: String,
    name: String,
    text: String,
    #[serde(rename = "isMine")]
    is_mine: Option<bool>,
}

#[derive(Clone)]
struct AppState {
    messages: Arc<RwLock<Vec<Message>>>,
}

struct App {
    state: AppState,
}

impl App {
    pub fn new() -> App {
        let state = AppState {
            messages: Arc::new(RwLock::new(Vec::new())),
        };

        Self { state }
    }

    async fn messages_get_handler(State(state): State<AppState>) -> Json<Vec<Message>> {
        let messages = state.messages.read().await;
        Json(messages.clone())
    }

    async fn messages_post_handler(
        State(state): State<AppState>,
        Json(message): Json<Message>,
    ) -> impl IntoResponse {
        let mut messages = state.messages.write().await;
        messages.push(message);
        println!("{:?}", messages);
        (StatusCode::CREATED, "Message created")
    }

    async fn fallback_handler() -> impl IntoResponse {
        (StatusCode::NOT_FOUND, "Page not found")
    }

    pub async fn listen(&self, addr: SocketAddr) {
        println!("Listened at http://{}", addr);

        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

        let app = Router::new()
            .nest_service("/assets", ServeDir::new("./assets"))
            .route_service("/", ServeFile::new("views/index.html"))
            .route_service("/error", ServeFile::new("views/error.html"))
            .route(
                "/messages",
                get(Self::messages_get_handler).post(Self::messages_post_handler),
            )
            .with_state(self.state.clone())
            .fallback(Self::fallback_handler);

        axum::serve(listener, app).await.unwrap();
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let app = App::new();
    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));

    app.listen(addr).await;
}
