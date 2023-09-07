use crate::CostFunction;

pub struct StaticCost {
    cost: f64
}

impl StaticCost {
    pub fn new(cost: f64) -> Self {
        Self {
            cost
        }
    }
}

impl CostFunction for StaticCost {
    fn cost(&self, _: &crate::ReservationParameters, _: &chrono::DateTime<chrono::Utc>) -> f64 {
        self.cost
    }
}