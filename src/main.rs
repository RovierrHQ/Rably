use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::IntoResponse,
    routing::get,
    Router,
};
use dashmap::DashMap;
use futures::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use uuid::Uuid;

// Application state shared across connections
#[derive(Clone)]
struct AppState {
    // Map channel_id -> broadcast sender for that channel
    channels: Arc<DashMap<String, broadcast::Sender<String>>>,
    // Track active connections per channel for presence
    channel_presence: Arc<DashMap<String, DashMap<String, ClientInfo>>>,
}

// Client connection info for presence tracking
#[derive(Clone, Debug, Serialize)]
struct ClientInfo {
    id: String,
    role: String, // "teacher" or "student"
    joined_at: i64,
}

// Incoming messages from WebSocket clients
#[derive(Deserialize, Debug)]
struct ClientMessage {
    action: String,
    channel: String,
    data: Option<serde_json::Value>,
    role: Option<String>, // "teacher" or "student"
}

// Outgoing messages to WebSocket clients
#[derive(Serialize, Debug)]
struct ServerMessage {
    r#type: String,
    channel: String,
    data: serde_json::Value,
    timestamp: i64,
}

#[tokio::main]
async fn main() {
    // Initialize logging
    env_logger::init();

    let state = AppState {
        channels: Arc::new(DashMap::new()),
        channel_presence: Arc::new(DashMap::new()),
    };

    // Build the router with CORS support
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/health", get(health_check))
        .route("/channels/{channel_id}/presence", get(get_channel_presence))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let addr = SocketAddr::from(([0, 0, 0, 0], port.parse().unwrap()));

    println!("🚀 Rably WebSocket server starting on {}", addr);
    println!("📡 WebSocket endpoint: ws://localhost:{}/ws", port);
    println!("🏥 Health check: http://localhost:{}/health", port);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

// Health check endpoint
async fn health_check() -> impl IntoResponse {
    serde_json::json!({
        "status": "healthy",
        "service": "rably-websocket",
        "timestamp": chrono::Utc::now().timestamp()
    }).to_string()
}

// Get presence info for a channel
async fn get_channel_presence(
    axum::extract::Path(channel_id): axum::extract::Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let presence = state
        .channel_presence
        .get(&channel_id)
        .map(|channel_map| {
            channel_map.iter().map(|entry| entry.value().clone()).collect::<Vec<_>>()
        })
        .unwrap_or_default();

    serde_json::json!({
        "channel": channel_id,
        "participants": presence
    }).to_string()
}

// WebSocket upgrade handler
async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

// Handle individual WebSocket connection
async fn handle_socket(socket: WebSocket, state: AppState) {
    let client_id = Uuid::new_v4().to_string();
    let (sender, mut receiver) = socket.split();

    println!("🔌 Client {} connected", client_id);

            // Create a channel for outgoing messages
    let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Spawn task to handle outgoing messages
    let sender_handle = {
        let mut sender = sender;
        tokio::spawn(async move {
            while let Some(msg) = outgoing_rx.recv().await {
                if sender.send(Message::Text(msg.into())).await.is_err() {
                    break;
                }
            }
        })
    };

    // Handle incoming messages
    while let Some(Ok(msg)) = receiver.next().await {
        if let Message::Text(text) = msg {
            if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                match client_msg.action.as_str() {
                    "subscribe" => {
                        let channel = client_msg.channel.clone();

                        // Get or create broadcast sender for this channel
                        let tx = state.channels
                            .entry(channel.clone())
                            .or_insert_with(|| broadcast::channel(1000).0)
                            .clone();

                        // Subscribe to the channel and forward messages
                        let mut rx = tx.subscribe();
                        let outgoing_tx_clone = outgoing_tx.clone();

                        tokio::spawn(async move {
                            while let Ok(msg) = rx.recv().await {
                                if outgoing_tx_clone.send(msg).is_err() {
                                    break;
                                }
                            }
                        });

                        // Add to presence tracking
                        let client_info = ClientInfo {
                            id: client_id.clone(),
                            role: client_msg.role.unwrap_or_else(|| "student".to_string()),
                            joined_at: chrono::Utc::now().timestamp(),
                        };

                        state.channel_presence
                            .entry(channel.clone())
                            .or_insert_with(DashMap::new)
                            .insert(client_id.clone(), client_info.clone());

                        // Notify channel of new participant
                        let presence_msg = ServerMessage {
                            r#type: "user_joined".to_string(),
                            channel: channel.clone(),
                            data: serde_json::to_value(&client_info).unwrap(),
                            timestamp: chrono::Utc::now().timestamp(),
                        };

                        if let Ok(msg_str) = serde_json::to_string(&presence_msg) {
                            let _ = tx.send(msg_str);
                        }

                        println!("📋 Client {} subscribed to channel {}", client_id, channel);
                    }

                    "publish" => {
                        let channel = client_msg.channel.clone();

                        if let Some(tx) = state.channels.get(&channel) {
                            let server_msg = ServerMessage {
                                r#type: "message".to_string(),
                                channel: channel.clone(),
                                data: client_msg.data.unwrap_or(serde_json::json!({})),
                                timestamp: chrono::Utc::now().timestamp(),
                            };

                            if let Ok(msg_str) = serde_json::to_string(&server_msg) {
                                let _ = tx.send(msg_str);
                                println!("📡 Message published to channel {} by client {}", channel, client_id);
                            }
                        }
                    }

                    "slide_change" => {
                        // Special handling for slide changes (core feature)
                        let channel = client_msg.channel.clone();

                        if let Some(tx) = state.channels.get(&channel) {
                            let slide_msg = ServerMessage {
                                r#type: "slide_change".to_string(),
                                channel: channel.clone(),
                                data: client_msg.data.unwrap_or(serde_json::json!({})),
                                timestamp: chrono::Utc::now().timestamp(),
                            };

                            if let Ok(msg_str) = serde_json::to_string(&slide_msg) {
                                let _ = tx.send(msg_str);
                                println!("🎯 Slide change broadcast to channel {} by client {}", channel, client_id);
                            }
                        }
                    }

                    _ => {
                        println!("❓ Unknown action: {} from client {}", client_msg.action, client_id);
                    }
                }
            }
        } else if let Message::Close(_) = msg {
            println!("🔌 Client {} requested close", client_id);
            break;
        }
    }

    // Cleanup
    sender_handle.abort();

        println!("🔌 Client {} disconnected", client_id);
}
