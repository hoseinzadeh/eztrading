use chrono::{DateTime, Utc};
use com::data::Ticker;
use com::failed;
use com::net::{WebSocketServer, Route, Symbol};
use com::status::Status;
use crossbeam_channel::Sender;
use futures::StreamExt;
use futures::{FutureExt, SinkExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use crate::providers::twelvedata::{TwelveData, WSValue};

#[derive(Deserialize, Serialize, Debug)]
pub struct WSParams {
    token: Option<String>, //TODO: no empty
    user: Option<String>,  //TODO: no empty
    pub(crate) symbols: Option<String>,
}

#[allow(dead_code)]
pub(crate) async fn websocket() {
    use warp::ws::WebSocket;
    use warp::Filter;
    async fn handle_client(ws: WebSocket) {
        // Split the socket into a sender and receive of messages.
        let (mut client_tx, _) = ws.split();
        // let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        // let mut rx = UnboundedReceiverStream::new(rx);

        loop {
            let msg = Into::<DateTime<Utc>>::into(SystemTime::now())
                .format("%d/%m/%Y %T")
                .to_string();
            match client_tx.send(warp::ws::Message::text(msg)).await {
                Ok(_) => (),
                Err(e) => {
                    eprintln!("/clock: websocket send error: {}", e);
                    break;
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await
        }

        eprintln!("/clock: websocket error: ConnectionClosed");
    }

    let clock = warp::path("clock")
        .and(warp::query::<WSParams>())
        .and(warp::ws())
        .map(|params: WSParams, ws: warp::ws::Ws| {
            println!("/clock: {:?}", params);

            ws.on_upgrade(move |ws| handle_client(ws))
        });

    let demo = warp::path("demo")
        .and(warp::query::<WSParams>())
        .and(warp::ws())
        .map(|params: WSParams, ws: warp::ws::Ws| {
            println!("/demo: {:?}", params);

            ws.on_upgrade(|websocket| {
                let (tx, rx) = websocket.split();
                rx.forward(tx).map(|result| {
                    if let Err(e) = result {
                        eprintln!("/demo: websocket error: {:?}", e);
                    }
                })
            })
        });

    warp::serve(warp::get().and(demo.or(clock)))
        // .tls()
        // .cert_path("secret/cert.pem")
        // .key_path("secret/key.rsa")
        .run(([0, 0, 0, 0], 9231))
        .await;
}
