use std::{
    collections::{BTreeMap, HashMap},
    hash::Hash,
};

use chrono::{DateTime, Utc};
use ordered_float::OrderedFloat;
use pathfinding::{kuhn_munkres, prelude::Weights};
use term_table::{row::Row, table_cell::TableCell, Table};

use crate::{ReservationParameters, ReservationRequest, StartTimeRange};

struct SparseAxisMasker {
    masked_idx_remapping: Vec<Option<usize>>,
    len: usize,
}

impl SparseAxisMasker {
    pub fn init(num: usize) -> Self {
        Self {
            masked_idx_remapping: vec![None; num],
            len: 0,
        }
    }

    pub fn grow(&mut self, num: usize) {
        self.masked_idx_remapping
            .resize(self.masked_idx_remapping.len() + num, None);
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn remap(&self, idx: usize) -> Result<usize, &'static str> {
        if idx > self.masked_idx_remapping.len() {
            return Err("Index out of bounds");
        }

        if let Some(op) = self.masked_idx_remapping[idx] {
            return Ok(op);
        } else {
            return Err("Index is out of mask bounds");
        }
    }

    pub fn clear_and_rebuild_view(&mut self, vec: &mut Vec<usize>) {
        vec.sort();
        vec.dedup();
        let mut max_value_idx: usize = 0;
        let mut cum_idx: usize = 0;
        for i in 0..self.masked_idx_remapping.len() {
            if max_value_idx >= vec.len() {
                self.masked_idx_remapping[cum_idx] = Some(i);
                cum_idx += 1;
                continue;
            }

            if vec[max_value_idx] == i {
                self.masked_idx_remapping[cum_idx] = Some(i);
                max_value_idx += 1;
                continue;
            }

            self.masked_idx_remapping[cum_idx] = Some(i);
            cum_idx += 1;
        }
        self.len = cum_idx;
    }
}

#[cfg(test)]
#[test]
fn test_sparse_axis_masker() {
    let mut skip_indices: Vec<usize> = vec![3, 8];
    let arr: Vec<usize> = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9];

    let mut masker = SparseAxisMasker::init(arr.len());
    masker.clear_and_rebuild_view(&mut skip_indices);

    let expected: Vec<_> = arr
        .iter()
        .filter(|p| !skip_indices.contains(p))
        .map(|p| *p)
        .collect();
    let mut res = vec![];
    for i in 0..masker.len() {
        res.push(arr[masker.remap(i).unwrap()]);
    }

    assert_eq!(expected, res);
}

/// The Kuhn-Munkres algorithm provides the simplest type of reservation system.
/// In this reservation system, time is ignored,
/// rather the best assignments between resources. An example use case for this
/// is in a fire emergency system where for instance robots.
/// This class also comes with a masker that allows you to add simple constraints while
/// solving an optimization.
struct ReservationsKuhnMunkres {
    resources: Vec<String>,
    resource_name_to_id: HashMap<String, usize>,
    requests: HashMap<usize, Vec<ReservationRequest>>,
    // Maps requests by (request_id, resource_id) -> index in requests table
    request_reservation_idx: HashMap<(usize, usize), usize>,
    last_request_id: usize,
    max_cost: f64,
    resource_mask: SparseAxisMasker,
    request_mask: SparseAxisMasker,
}

impl Weights<OrderedFloat<f64>> for ReservationsKuhnMunkres {
    fn rows(&self) -> usize {
        self.request_mask.len()
    }

    fn columns(&self) -> usize {
        self.resource_mask.len()
    }

    fn at(&self, row: usize, col: usize) -> OrderedFloat<f64> {
        let Ok(row) = self.request_mask.remap(row) else {
            return OrderedFloat(-self.max_cost - 1.0);
        };

        let Ok(col) = self.resource_mask.remap(col) else {
            return OrderedFloat(-self.max_cost - 1.0);
        };

        // Rows are the request that was made by the agent
        let Some(requests) = self.requests.get(&row) else {
            return OrderedFloat(-self.max_cost - 1.0);
        };

        // Columns are the resources
        let Some(&request_idx) = self.request_reservation_idx.get(&(row, col)) else {
            return OrderedFloat(-self.max_cost - 1.0);
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
    fn print_debug_matrix(&self) {
        let mut table = Table::new();

        let mut header = vec![TableCell::new("Request id")];

        let columns = (0..self.columns())
            .map(|idx| self.resource_mask.remap(idx))
            .map(|idx| self.resources[idx.unwrap()].clone())
            .map(|resource| TableCell::new(resource));

        header.extend(columns);

        table.add_row(Row::new(header));

        for row in 0..self.request_mask.len() {
            let mut t_row = vec![TableCell::new(format!("{}", row))];

            for col in 0..self.resource_mask.len() {
                t_row.push(TableCell::new(format!("{}", self.at(row, col))));
            }
            table.add_row(Row::new(t_row));
        }
        println!("{}", table.render());
    }
    pub fn create_with_resources(resources: &Vec<String>) -> Self {
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
            resource_mask: SparseAxisMasker::init(resources.len()),
            request_mask: SparseAxisMasker::init(0),
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
        self.last_request_id += 1;
        self.requests.insert(req_id, request);
        self.request_mask.grow(1);
        Some(req_id)
    }

    pub fn solve(&mut self) -> (OrderedFloat<f64>, HashMap<usize, Option<usize>>) {
        let mut res = HashMap::new();
        let mut mask = vec![];
        self.request_mask.clear_and_rebuild_view(&mut mask);
        self.resource_mask.clear_and_rebuild_view(&mut mask);

        let (cost, results) = kuhn_munkres::kuhn_munkres(self);
        for row_idx in 0..results.len() {
            let col_idx = results[row_idx];
            let Some(req_id) = self.request_reservation_idx.get(&(row_idx, col_idx)) else {
                // Failed to allocate any valid option
                // System is probably over subscribed
                res.insert(row_idx, None);
                continue;
            };
            res.insert(row_idx, Some(*req_id));
        }
        (-cost, res)
    }

    /// constraints are of the form (request_id,
    /// constraint that should be satisfied by request)
    /// (resource_id)
    pub fn solve_with_constraint(
        &mut self,
        positive_constraints: Vec<(usize, usize)>,
        removed_resources: Vec<usize>,
    ) -> (OrderedFloat<f64>, HashMap<usize, Option<usize>>) {
        let mut resource_mask = vec![];
        let mut request_mask = vec![];
        for (request_id, satisfied_resource) in positive_constraints {
            if let Some(reservations) = self.requests.get(&request_id) {
                let Some(res) = reservations.get(satisfied_resource) else {
                    continue;
                };
                request_mask.push(request_id);

                let Some(&res) = self.resource_name_to_id.get(&res.parameters.resource_name) else {
                    continue;
                };
                resource_mask.push(res);
            }
        }

        for resource in removed_resources {
            resource_mask.push(resource);
        }

        let mut res = HashMap::new();
        self.request_mask.clear_and_rebuild_view(&mut request_mask);
        self.resource_mask
            .clear_and_rebuild_view(&mut resource_mask);
        let (cost, results) = kuhn_munkres::kuhn_munkres(self);

        for row_idx in 0..results.len() {
            let Some(req) = self.requests.get(&row_idx) else {
                continue;
            };
            let col_idx = results[row_idx];
            let Ok(row_idx) = self.request_mask.remap(row_idx) else {
                continue;
            };
            let Ok(col_idx) = self.resource_mask.remap(col_idx) else {
                continue;
            };
            let Some(req_id) = self.request_reservation_idx.get(&(row_idx, col_idx)) else {
                // Failed to allocate any valid option
                // System is probably over subscribed
                res.insert(row_idx, None);
                continue;
            };
            res.insert(row_idx, Some(*req_id));
        }
        (-cost, res)
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
    let mut res_sys = ReservationsKuhnMunkres::create_with_resources(&resources);

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

    let (cost, res) = res_sys.solve();

    res_sys.print_debug_matrix();

    let r1 = res[&idx1.unwrap()].unwrap();
    let r2 = res[&idx2.unwrap()].unwrap();

    println!("cost {}", cost);
    assert!((cost.0 - 2.0).abs() < 1e-9);
    assert_eq!(r1, 1);
    assert_eq!(r2, 1);
}

#[cfg(test)]
#[test]
fn test_resource_constraint() {
    use std::sync::Arc;

    use crate::cost_function::static_cost::StaticCost;

    let resources: Vec<_> = vec!["Parking Spot 1", "Parking Spot 2", "Parking Spot 3"]
        .iter()
        .map(|c| c.to_string())
        .collect();
    let mut res_sys = ReservationsKuhnMunkres::create_with_resources(&resources);

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

    let mut removed_resources = vec![1];
    let (cost, res) = res_sys.solve_with_constraint(vec![], removed_resources);

    println!("{:?}", res);
    res_sys.print_debug_matrix();

    let r1 = res[&idx1.unwrap()].unwrap();
    let r2 = res[&idx2.unwrap()].unwrap();

    println!("cost {cost}");

    assert_eq!(r1, 0);
    assert_eq!(r2, 1);
}

#[cfg(test)]
#[test]
fn test_rquest_constraint() {
    use std::sync::Arc;

    use crate::cost_function::static_cost::StaticCost;

    let resources: Vec<_> = vec!["Parking Spot 1", "Parking Spot 2", "Parking Spot 3"]
        .iter()
        .map(|c| c.to_string())
        .collect();
    let mut res_sys = ReservationsKuhnMunkres::create_with_resources(&resources);

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

    let mut removed_resources = vec![1];
    let (cost, res) = res_sys.solve_with_constraint(vec![], removed_resources);

    println!("{:?}", res);
    res_sys.print_debug_matrix();

    let r1 = res[&idx1.unwrap()].unwrap();
    let r2 = res[&idx2.unwrap()].unwrap();

    println!("cost {cost}");

    assert_eq!(r1, 0);
    assert_eq!(r2, 1);
}
