use std::net::SocketAddr;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{RwLock, mpsc};
use uuid::Uuid;
use warp::ws::{Message, WebSocket};
use warp::{Filter, ws::Ws};

#[derive(Deserialize, Serialize, Debug, Clone)]
struct MessageData {
    color: String,
    name: String,
    text: String,
    #[serde(rename = "isMine")]
    is_mine: Option<bool>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
struct WebSocketMessage<T> {
    #[serde(rename = "type")]
    msg_type: String,
    data: T,
}

#[derive(Deserialize, Serialize, Debug)]
struct NewMessageData {
    name: String,
    text: String,
    color: String,
}

type ClientId = String;

#[derive(Clone)]
struct AppState {
    messages: Arc<RwLock<Vec<MessageData>>>,
    clients: Arc<RwLock<Vec<ClientSender>>>,
}

#[derive(Clone)]
struct ClientSender {
    id: ClientId,
    sender: mpsc::UnboundedSender<serde_json::Value>,
}

struct App {
    state: AppState,
}

impl App {
    pub fn new() -> App {
        let state = AppState {
            messages: Arc::new(RwLock::new(Vec::new())),
            clients: Arc::new(RwLock::new(Vec::new())),
        };

        Self { state }
    }

    pub async fn listen(&self, addr: SocketAddr) {
        println!("Server running at http://{}", addr);
        println!("WebSocket available at ws://{}/ws", addr);

        let state = self.state.clone();

        let assets = warp::path("assets").and(warp::fs::dir("./assets"));
        let index = warp::path::end().and(warp::fs::file("views/index.html"));
        let error = warp::path("error").and(warp::fs::file("views/error.html"));

        let ws_route = warp::path("ws")
            .and(warp::ws())
            .and(with_state(state.clone()))
            .map(|ws: Ws, state: AppState| {
                ws.on_upgrade(move |socket| handle_connection(socket, state))
            });

        let fallback = warp::any()
            .map(|| warp::reply::with_status("Page not found", warp::http::StatusCode::NOT_FOUND));

        let routes = assets.or(index).or(error).or(ws_route).or(fallback);

        warp::serve(routes).run(addr).await;
    }
}

fn with_state(
    state: AppState,
) -> impl Filter<Extract = (AppState,), Error = std::convert::Infallible> + Clone {
    warp::any().map(move || state.clone())
}

async fn handle_connection(ws: WebSocket, state: AppState) {
    println!("New WebSocket connection");

    let (mut ws_tx, mut ws_rx) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel();

    let client_id = Uuid::new_v4().to_string();

    {
        let client_sender = ClientSender {
            id: client_id.clone(),
            sender: tx.clone(),
        };
        state.clients.write().await.push(client_sender);
    }

    {
        let messages = state.messages.read().await;
        let welcome_message = json!({
            "type": "get_messages",
            "data": &*messages
        });

        if let Err(e) = tx.send(welcome_message) {
            eprintln!("Failed to send welcome message: {}", e);
        }
    }

    let send_task = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if let Ok(text) = serde_json::to_string(&message) {
                if ws_tx.send(Message::text(text)).await.is_err() {
                    break;
                }
            }
        }
    });

    let recv_state = state.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(result) = ws_rx.next().await {
            match result {
                Ok(msg) => {
                    if msg.is_text() {
                        handle_client_message(&client_id, msg.to_str().unwrap(), &recv_state).await;
                    }
                }
                Err(e) => {
                    eprintln!("WebSocket error: {}", e);
                    break;
                }
            }
        }

        recv_state
            .clients
            .write()
            .await
            .retain(|c| c.id != client_id);
        println!("Client disconnected: {}", client_id);
    });

    let _ = tokio::join!(send_task, recv_task);
}

async fn handle_client_message(client_id: &str, msg: &str, state: &AppState) {
    println!("Received from client {}: {}", client_id, msg);

    let ws_message: Result<WebSocketMessage<serde_json::Value>, _> = serde_json::from_str(msg);

    match ws_message {
        Ok(msg) => {
            match msg.msg_type.as_str() {
                "new_message" => {
                    match serde_json::from_value::<NewMessageData>(msg.data.clone()) {
                        Ok(new_message_data) => {
                            let message = MessageData {
                                color: new_message_data.color,
                                name: new_message_data.name,
                                text: new_message_data.text,
                                is_mine: Some(false),
                            };

                            state.messages.write().await.push(message.clone());
                            println!("New message added from client {}", client_id);

                            let state_clone = state.clone();
                            tokio::spawn(async move {
                                let message_json = json!({
                                    "type": "get_messages",
                                    "data": vec![&message]
                                });

                                let clients = state_clone.clients.read().await;
                                for client in clients.iter() {
                                    if let Err(e) = client.sender.send(message_json.clone()) {
                                        eprintln!(
                                            "Failed to broadcast to client {}: {}",
                                            client.id, e
                                        );
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            eprintln!("Failed to parse new_message data: {}", e);
                        }
                    }
                }
                "get_messages" => {
                    let messages = state.messages.read().await;
                    let response = json!({
                        "type": "get_messages",
                        "data": &*messages
                    });

                    let clients = state.clients.read().await;
                    if let Some(client) = clients.iter().find(|c| c.id == client_id) {
                        if let Err(e) = client.sender.send(response) {
                            eprintln!("Failed to send messages to client {}: {}", client_id, e);
                        }
                    }
                }
                _ => {
                    println!("Unknown message type: {}", msg.msg_type);
                }
            }
        }
        Err(e) => {
            eprintln!("Failed to parse WebSocket message: {}", e);
        }
    }
}

#[tokio::main]
async fn main() {
    let app = App::new();
    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));

    app.listen(addr).await;
}
