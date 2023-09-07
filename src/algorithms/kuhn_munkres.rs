use std::{collections::HashMap, hash::Hash};

use chrono::{DateTime, Utc};
use ordered_float::OrderedFloat;
use pathfinding::{kuhn_munkres, prelude::Weights};

use crate::{ReservationParameters, ReservationRequest, StartTimeRange};

struct ReservationsKuhnMunkres {
    resources: Vec<String>,
    resource_name_to_id: HashMap<String, usize>,
    requests: HashMap<usize, Vec<ReservationRequest>>,
    // Maps requests by (request_id, resource_id) -> index in requests table
    request_reservation_idx: HashMap<(usize, usize), usize>,
    last_request_id: usize,
    max_cost: f64,
}

impl Weights<OrderedFloat<f64>> for ReservationsKuhnMunkres {
    fn rows(&self) -> usize {
        self.last_request_id.max(self.requests.len())
    }

    fn columns(&self) -> usize {
        self.last_request_id.max(self.resources.len())
    }

    fn at(&self, row: usize, col: usize) -> OrderedFloat<f64> {
        // Rows are the request that was made by the agent
        let Some(requests) = self.requests.get(&row) else {
            return OrderedFloat(- self.max_cost - 1.0);
        };

        // Columns are the resources
        let Some(&request_idx) = self.request_reservation_idx.get(&(row, col)) else {
            return OrderedFloat(- self.max_cost - 1.0);
        };

        // TODO(arjo) handle different costs based on allocation. For now pass rubbish.
        return OrderedFloat(
            -requests[request_idx]
                .cost_function
                .cost(&requests[request_idx].parameters, &DateTime::<Utc>::MIN_UTC),
        );
    }

    fn neg(&self) -> Self
    where
        Self: Sized,
    {
        todo!()
    }
}

impl ReservationsKuhnMunkres {
    pub fn create_with_resources(resources: Vec<String>) -> Self {
        Self {
            resources: resources.clone(),
            resource_name_to_id: HashMap::from_iter(
                resources
                    .iter()
                    .enumerate()
                    .map(|(size, str)| (str.clone(), size)),
            ),
            requests: HashMap::new(),
            max_cost: 0.0,
            last_request_id: 0,
            request_reservation_idx: HashMap::new(),
        }
    }

    pub fn request_resources(&mut self, request: Vec<ReservationRequest>) -> Option<usize> {
        let req_id = self.last_request_id;
        for r_id in 0..request.len() {
            let resource = request[r_id].parameters.resource_name.clone();
            let Some(&resource_id) = self.resource_name_to_id.get(&resource) else {
                return None; 
            };
            self.request_reservation_idx
                .insert((req_id, resource_id), r_id);

            self.max_cost = self.max_cost.max(
                request[r_id]
                    .cost_function
                    .cost(&request[r_id].parameters, &DateTime::<Utc>::MIN_UTC),
            );
        }

        self.requests.insert(req_id, request);
        Some(req_id)
    }

    pub fn solve(&self) -> HashMap<usize, Option<usize>> {
        let mut res = HashMap::new();
        let (_, results) = kuhn_munkres::kuhn_munkres(self);
        for row_idx in 0..results.len() {
            let Some(req) = self.requests.get(&row_idx) else {
                continue;
            };
            let col_idx = results[row_idx];
            let Some(req_id) = self.request_reservation_idx.get(&(row_idx, col_idx)) else {
                // Failed to allocate any valid option
                // System is probably over subscribed
                res.insert(row_idx, None);
                continue;
            };
            res.insert(row_idx, Some(*req_id));
        }
        res
    }
}

#[cfg(test)]
#[test]
fn test_kuhn_munkres_correctness() {
    use std::sync::Arc;

    use crate::cost_function::static_cost::StaticCost;

    let resources: Vec<_> = vec!["Parking Spot 1", "Parking Spot 2", "Parking Spot 3"]
        .iter()
        .map(|c| c.to_string())
        .collect();
    let mut res_sys = ReservationsKuhnMunkres::create_with_resources(resources);

    let req1 = vec![
        ReservationRequest {
            parameters: ReservationParameters {
                resource_name: "Parking Spot 1".to_string(),
                duration: None,
                start_time: StartTimeRange::no_specifics(),
            },
            cost_function: Arc::new(StaticCost::new(10.0)),
        },
        ReservationRequest {
            parameters: ReservationParameters {
                resource_name: "Parking Spot 2".to_string(),
                duration: None,
                start_time: StartTimeRange::no_specifics(),
            },
            cost_function: Arc::new(StaticCost::new(1.0)),
        },
    ];
    let req2 = vec![
        ReservationRequest {
            parameters: ReservationParameters {
                resource_name: "Parking Spot 2".to_string(),
                duration: None,
                start_time: StartTimeRange::no_specifics(),
            },
            cost_function: Arc::new(StaticCost::new(10.0)),
        },
        ReservationRequest {
            parameters: ReservationParameters {
                resource_name: "Parking Spot 3".to_string(),
                duration: None,
                start_time: StartTimeRange::no_specifics(),
            },
            cost_function: Arc::new(StaticCost::new(1.0)),
        },
    ];

    let idx1 = res_sys.request_resources(req1);
    let idx2 = res_sys.request_resources(req2);

    let res = res_sys.solve();

    let r1 = res[&idx1.unwrap()].unwrap();
    let r2 = res[&idx2.unwrap()].unwrap();

    assert_eq!(r1, 1);
    assert_eq!(r2, 1);
}
