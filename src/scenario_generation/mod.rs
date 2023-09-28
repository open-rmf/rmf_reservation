use std::{collections::HashMap, sync::Arc};

use chrono::{Duration, Utc, TimeZone};
use rand::Rng;

use crate::{ReservationRequest, ReservationSchedule, cost_function::static_cost::StaticCost};

// TODO(arjo) there is bug in this that generates rubbish occassionally.
pub fn generate_test_scenario_with_known_best(
    num_resources: usize,
    max_requests_per_resource: usize,
    num_conflicts: usize,
) -> (Vec<Vec<ReservationRequest>>, Vec<String>) {
    let resources = Vec::from_iter((0..num_resources).map(|m| format!("Station {}", m)));
    let mut rng = rand::thread_rng();

    let mut results_vec = vec![];

    let mut reservations_schedule = HashMap::new();
    // Build correct solution
    for res in &resources {
        let res_size = rng.gen_range(0..=max_requests_per_resource);

        let start_time = Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap();
        let mut last_start_time = start_time;
        reservations_schedule.insert(res.clone(), ReservationSchedule::new());
        for i in 0..res_size {
            last_start_time += Duration::minutes(rng.gen_range(0..60));
            let duration = Duration::minutes(rng.gen_range(20..90));
            let request = ReservationRequest {
                parameters: crate::ReservationParameters {
                    resource_name: res.clone(),
                    duration: Some(duration),
                    start_time: crate::StartTimeRange {
                        earliest_start: Some(last_start_time),
                        latest_start: Some(last_start_time),
                    },
                },
                cost_function: Arc::new(StaticCost::new(1.0)),
            };

            let Some(schedule )= reservations_schedule.get_mut(res) else {
                panic!("Should never get here");
            };
            schedule.insert((i, 0), Some(duration), last_start_time);
            last_start_time += duration;

            results_vec.push(vec![request]);
        }
    }

    // Add conflicts for fun at lower cost
    for _ in 0..num_conflicts {
        let res_id = rng.gen_range(0..resources.len());
        let resource_name = &resources[res_id];
        let Some(schedule) = reservations_schedule.get(resource_name) else {
            continue;
        };

        if schedule.schedule.len() == 0 {
            continue;
        }

        let res_id = if schedule.schedule.len() > 1 {
            rng.gen_range(1..schedule.schedule.len())
        } else {
            1
        };
        let mut idx = 0;
        let mut prev_time = *schedule.schedule.first_key_value().unwrap().0;
        for s in &schedule.schedule {
            if idx == res_id {
                // Lets create a reservation to disrupt the previous two reservations
                let req = ReservationRequest {
                    parameters: crate::ReservationParameters {
                        resource_name: resource_name.clone(),
                        duration: Some((*s.0 + s.1 .2.unwrap()) - prev_time),
                        start_time: crate::StartTimeRange::exactly_at(&prev_time),
                    },
                    cost_function: Arc::new(StaticCost::new(0.9)),
                };
                let rn = rng.gen_range(0..results_vec.len());
                results_vec[rn].push(req);
            }
            idx += 1;
            prev_time = *s.0;
        }
    }

    (results_vec, resources)
}



pub fn save_scenario_assume_static_cost(
    requests: &Vec<Vec<ReservationRequest>>,
    reservation: &Vec<String>) {
    
}