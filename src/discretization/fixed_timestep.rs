use std::{collections::HashMap, sync::Arc};

use chrono::{DateTime, Duration, Utc};

use crate::{cost_function::static_cost::StaticCost, ReservationRequest, StartTimeRange};

use super::DescretizationStrategy;

pub struct FixedTimestamp {
    remapping: HashMap<(usize, usize), (usize, usize)>,
    timestep: Duration,
    latest: chrono::DateTime<Utc>,
}

impl FixedTimestamp {
    fn new(timestep: Duration, latest: chrono::DateTime<Utc>) -> Self {
        Self {
            remapping: HashMap::new(),
            timestep,
            latest,
        }
    }
}

impl DescretizationStrategy for FixedTimestamp {
    fn discretize(
        &mut self,
        requests: &Vec<Vec<crate::ReservationRequest>>,
    ) -> Vec<Vec<crate::ReservationRequest>> {
        let mut result = vec![];

        for request_id in 0..requests.len() {
            let mut broken_down_options = vec![];
            for res in 0..requests[request_id].len() {
                let earliest = if let Some(earliest) = requests[request_id][res]
                    .parameters
                    .start_time
                    .earliest_start
                {
                    earliest
                } else {
                    Utc::now()
                };

                let latest = if let Some(latest) =
                    requests[request_id][res].parameters.start_time.latest_start
                {
                    latest
                } else {
                    self.latest
                };

                let mut current_time = earliest;

                while current_time != latest {
                    let mut params = requests[request_id][res].parameters.clone();
                    params.start_time = StartTimeRange::exactly_at(&current_time);
                    let cost = requests[request_id][res]
                        .cost_function
                        .cost(&params, &current_time);

                    let new_reservation = ReservationRequest {
                        parameters: params,
                        cost_function: Arc::new(StaticCost::new(cost)),
                    };
                    broken_down_options.push(new_reservation);
                    current_time += self.timestep;
                }
            }
            result.push(broken_down_options);
        }
        //self.remapping
        result
    }

    fn remap(&self, ticket_id: &(usize, usize)) -> (usize, usize) {
        todo!()
    }
}
