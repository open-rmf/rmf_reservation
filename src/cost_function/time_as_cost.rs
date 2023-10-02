
use chrono::{Utc, DateTime};

use crate::CostFunction;

/// Using this cost function will minimize the total time the reservation system spends
struct TimeAsCost {

}

impl CostFunction for TimeAsCost {
    fn cost(&self, _: &crate::ReservationParameters, instant: &chrono::DateTime<chrono::Utc>) -> f64 {
        (instant.signed_duration_since(DateTime::<Utc>::MIN_UTC).num_milliseconds() as f64)  / 1000.0
    }
}