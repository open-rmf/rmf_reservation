use std::{collections::HashMap, sync::Arc};

use chrono::{DateTime, Duration, Utc};

use crate::{
    cost_function::static_cost::StaticCost, database::ClockSource, ReservationRequestAlternative,
    StartTimeRange,
};

use super::DescretizationStrategy;

pub struct FixedTimestep<C: ClockSource> {
    remapping: HashMap<(usize, usize), (usize, usize)>,
    timestep: Duration,
    latest: chrono::DateTime<Utc>,
    clock_source: C,
}

impl<C: ClockSource> FixedTimestep<C> {
    pub fn new(timestep: Duration, latest: chrono::DateTime<Utc>, clock_source: C) -> Self {
        Self {
            remapping: HashMap::new(),
            timestep,
            latest,
            clock_source,
        }
    }
}

impl<C: ClockSource> DescretizationStrategy for FixedTimestep<C> {
    fn discretize(
        &mut self,
        requests: &Vec<Vec<crate::ReservationRequestAlternative>>,
    ) -> Vec<Vec<crate::ReservationRequestAlternative>> {
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
                    self.clock_source.now()
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

                    let new_reservation = ReservationRequestAlternative {
                        parameters: params,
                        cost_function: Arc::new(StaticCost::new(cost)),
                    };
                    broken_down_options.push(new_reservation);
                    current_time += self.timestep;

                    self.remapping.insert(
                        (result.len(), broken_down_options.len() - 1),
                        (request_id, res),
                    );
                }
            }
            result.push(broken_down_options);
        }
        result
    }

    fn remap(&self, ticket_id: &(usize, usize)) -> Option<(usize, usize)> {
        self.remapping.get(ticket_id).copied()
    }
}
