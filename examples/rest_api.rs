use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use rmf_reservations::{
    AsyncReservationSystem, CostFunction, ReservationParameters, ReservationRequest,
    ReservationVoucher,
};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, sync::Arc};

use tokio::sync::RwLock;

#[derive(Deserialize)]
struct JSONReservationRequest {
    choices: Vec<ReservationParameters>,
}

#[derive(Deserialize, Serialize)]
struct ReservationVoucherStamped {
    voucher: ReservationVoucher,
}

#[derive(Serialize)]
enum JSONReservationResponse {
    Ok(ReservationVoucherStamped),
    Error(String),
}

struct NoCost {}

impl CostFunction for NoCost {
    fn cost(&self, _parameters: &ReservationParameters, _instant: &DateTime<Utc>) -> f64 {
        0f64
    }
}

#[axum::debug_handler]
async fn reserve_item(
    State(reservation_system): State<Arc<RwLock<AsyncReservationSystem>>>,
    Json(payload): Json<JSONReservationRequest>,
) -> Json<JSONReservationResponse> {
    let reservations: Vec<_> = payload
        .choices
        .iter()
        .map(|parameters| -> ReservationRequest {
            ReservationRequest {
                parameters: parameters.clone(),
                cost_function: Arc::new(NoCost {}),
            }
        })
        .collect();
    let mut res = reservation_system.write().await;
    let result = res.request_reservation(reservations);
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
    State(reservation_system): State<Arc<RwLock<AsyncReservationSystem>>>,
    Json(payload): Json<ReservationVoucherStamped>,
) -> Json<Result<String, &'static str>> {
    let sys = reservation_system.write().await;
    if let Ok(location) = sys.claim(payload.voucher).await {
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

    let mut reservation_system = Arc::new(RwLock::new(AsyncReservationSystem::new(&resources)));

    let res_sys_thread = { reservation_system.read().await.spin_in_bg() };

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

    res_sys_thread.join();
}
