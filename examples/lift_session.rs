use std::sync::Arc;

use chrono::Utc;
use rmf_reservations::{
    algorithms::sat_flexible_time_model::Problem, cost_function::static_cost,
    ReservationParameters, ReservationRequestAlternative,
};

struct Robot {
    time_from_start_to_lift_travel: Vec<chrono::Duration>,
    time_from_lift_to_destination: Vec<chrono::Duration>,
    start_floor: usize,
    end_floor: usize,
}

struct Lift;

struct LiftAssigner {
    robots: Vec<Robot>,
    lifts: Vec<Lift>,
    lift_time_per_floor: chrono::Duration,
    start_time: chrono::DateTime<Utc>,
}

impl LiftAssigner {
    fn get_problem(&self) -> Problem {
        let mut p = Problem::default();

        for robot in &self.robots {
            //let lift_alternatives = vec![];
            for lift_id in 0..self.lifts.len() {
                let time_to_lift = robot.time_from_start_to_lift_travel[lift_id];
                let time_from_lift_to_dest = robot.time_from_lift_to_destination[lift_id];
                let diff = robot.start_floor.abs_diff(robot.end_floor);
                let time_in_lift = (0..diff).fold(chrono::Duration::new(0, 0).unwrap(), |a, _| {
                    a + self.lift_time_per_floor
                });
                /*let req = ReservationRequestAlternative {
                    cost_function: Arc::new(static_cost::StaticCost(1.0)),
                    parameters: ReservationParameters {
                        resource_name: ,
                        start_time:
                    }
                };
                lift_alternatives.push();*/
            }
        }

        p
    }
}

fn main() {
    let mut p = Problem::default();
}
