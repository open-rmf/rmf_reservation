use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use rmf_reservations::{
    cost_function::static_cost::StaticCost,
    database::{FixedTimeReservationSystem, Ticket},
    AsyncReservationSystem, CostFunction, ReservationParameters, ReservationRequest,
    ReservationVoucher,
};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, sync::Arc};

use tokio::sync::RwLock;

#[derive(Deserialize)]
struct JSONReservationRequest {
    choices: Vec<ReservationParameters>,
    costs: Vec<f64>,
}

#[derive(Deserialize, Serialize)]
struct ReservationVoucherStamped {
    voucher: Ticket,
}

#[derive(Serialize)]
enum JSONReservationResponse {
    Ok(ReservationVoucherStamped),
    Error(String),
}

#[axum::debug_handler]
async fn reserve_item(
    State(reservation_system): State<Arc<RwLock<FixedTimeReservationSystem>>>,
    Json(payload): Json<JSONReservationRequest>,
) -> Json<JSONReservationResponse> {
    let reservations: Vec<_> = payload
        .choices
        .iter()
        .enumerate()
        .map(|(idx, parameters)| -> ReservationRequest {
            ReservationRequest {
                parameters: parameters.clone(),
                cost_function: Arc::new(StaticCost::new(payload.costs[idx])),
            }
        })
        .collect();
    let mut res = reservation_system.write().await;
    let result = res.request_resources(reservations);
    if let Ok(voucher) = result {
        return Json(JSONReservationResponse::Ok(ReservationVoucherStamped {
            voucher,
        }));
    }

    Json(JSONReservationResponse::Error(
        "Failed to acquire lock".to_string(),
    ))
}

#[axum::debug_handler]
async fn claim(
    State(reservation_system): State<Arc<RwLock<FixedTimeReservationSystem>>>,
    Json(payload): Json<ReservationVoucherStamped>,
) -> Json<Result<usize, &'static str>> {
    let mut sys = reservation_system.write().await;
    if let Some(location) = sys.claim_request(payload.voucher) {
        Json(Ok(location))
    } else {
        Json(Err("Failed to claim"))
    }
}

#[tokio::main]
async fn main() {
    let resources = vec![
        "station1".to_string(),
        //"station2".to_string(),
        //"station3".to_string(),
    ];

    let mut reservation_system = Arc::new(RwLock::new(
        FixedTimeReservationSystem::create_with_resources(resources),
    ));

    let app = Router::new()
        .route("/reserve", post(reserve_item))
        .route("/claim", post(claim))
        .with_state(reservation_system.clone());

    let addr = SocketAddr::from(([127, 0, 0, 1], 4000));
    println!("listening on {}", addr);
    axum_server::bind(addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
