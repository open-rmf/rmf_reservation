use std::{collections::{HashMap, BTreeMap, HashSet, BinaryHeap}, hash::{Hash, self}, cmp::Ordering};

use chrono::{DateTime, Utc};
use ordered_float::OrderedFloat;

use crate::{ReservationRequest, utils::multimap::UniqueMultiHashMap};

struct FakeResourceMetaInfo {
    original_resource_name: String,
    original_resource_id: usize,
    original_request: (usize, usize),
}

pub struct TimeBasedBranchAndBound {
    base_resources: Vec<String>,
    last_request_id: usize,
    resource_name_to_id: HashMap<String, usize>,
    requests: HashMap<usize, Vec<ReservationRequest>>,
    // Maps requests by (request_id, resource_id) -> index in requests table
    request_reservation_idx: HashMap<(usize, usize), usize>,
}

impl TimeBasedBranchAndBound {
    pub fn create_with_resources(resources: &Vec<String>) -> Self {
        Self {
            base_resources: resources.clone(),
            last_request_id: 0,
            resource_name_to_id: HashMap::from_iter(
                resources
                    .iter()
                    .enumerate()
                    .map(|(size, str)| (str.clone(), size)),
            ),
            requests: HashMap::new(),
            request_reservation_idx: HashMap::new(),
        }
    }

    // Requires that ReservationRequests have a well defined start time
    pub fn request_resources(&mut self, request: Vec<ReservationRequest>) -> Option<usize> {
        let req_id = self.last_request_id;
        for r_id in 0..request.len() {
            let resource = request[r_id].parameters.resource_name.clone();
            let Some(&resource_id) = self.resource_name_to_id.get(&resource) else {
                return None; 
            };
            self.request_reservation_idx
                .insert((req_id, resource_id), r_id);
        }
        self.last_request_id += 1;
        self.requests.insert(req_id, request);
        Some(req_id)
    }

    fn generate_literals_and_remap_requests(&self) -> SparseScheduleConflictBranchAndBound {
        let mut fake_resources = vec![];
        let mut fake_resource_mapping: HashMap<String, FakeResourceMetaInfo> = HashMap::new();
        let mut fake_requests: HashMap<usize, Vec<ReservationRequest>> = HashMap::new();

        // HashMap<Resources, BTReeMAp<StartTime, Vec<(Req_id, alt_id, fake_resource_name, end_time)>>>
        let mut start_time_per_resource: HashMap<String, BTreeMap<DateTime<Utc>, Vec<(usize, usize, String, DateTime<Utc>)>>> = HashMap::new();

        for (req_id, requests) in &self.requests {
            let mut curr_req_alt = vec![];
            for i in 0..requests.len() {
                let request =  &requests[i];
                let resource_id = self.resource_name_to_id[&request.parameters.resource_name];
                
                // Generate new resource names
                let res_name = format!("{} {} {}", resource_id, req_id, i);
                fake_resources.push(res_name.clone());
                fake_resource_mapping.insert(res_name.clone(), FakeResourceMetaInfo {
                    original_resource_name: request.parameters.resource_name.clone(),
                    original_resource_id: resource_id,
                    original_request: (*req_id, i),
                });

                let mut fake_req = request.clone();
                fake_req.parameters.resource_name = res_name.clone();
                curr_req_alt.push(fake_req);


                // Build trees for calculating conflict
                // Note: we assume there is a well defined earliest start and that
                // start is the start time.
                let start = request.parameters.start_time.earliest_start.unwrap();

                // We also assume that all reservation requests come with a duration
                let end = start + request.parameters.duration.unwrap();

                if let Some(mut schedule ) = start_time_per_resource.get_mut(&request.parameters.resource_name) {
                    if let Some(mut bucket) = schedule.get_mut(&start) {
                        bucket.push((*req_id, i, res_name.clone(), end.clone()));
                    }
                    else{
                        schedule.insert(start.clone(), vec![(*req_id, i, res_name.clone(), end.clone())]);
                    }
                }
                else {
                    let mut btree = BTreeMap::new();
                    btree.insert(start.clone(), vec![(*req_id, i, res_name.clone(), end.clone())]);
                    start_time_per_resource.insert(request.parameters.resource_name.clone()
                        , btree);
                };
            }
            fake_requests.insert(*req_id, curr_req_alt);
        }

        // TODO(arjo) - This is very inefficient...
        let mut conflict_sets: HashMap<String, HashSet<String>> = HashMap::new();
        for resource in &self.base_resources {
            let Some(start_sched) = start_time_per_resource.get(resource) else {
                continue;
            };

            for (start_time, reservation) in start_sched {
                for (req_id, alt_id, res_name, end) in reservation {
                    
                    let mut conflicts = HashSet::new();

                    // We also assume that all reservation requests come with a duration
                    for (other_start_time, reservations) in start_sched {
                        if other_start_time > end {
                            break;
                        }
                        
                        for (other_req_id, other_alt_id, other_res_name, other_end) in reservations {
                            if other_res_name == res_name {
                                continue;
                            }
                            
                            // Case 1: Reservation starts during the current request in qn
                            if other_start_time >= start_time && other_start_time <= end {
                                conflicts.insert(other_res_name.clone());
                            }

                            // Case 2: Other start time is before my start time and ends after it
                            if other_start_time <= start_time && other_end >= start_time {
                                conflicts.insert(other_res_name.clone());
                            }
                        }
                    }
                    
                    for other_resource in &conflicts {
                        if let Some(mut conflict_set) = conflict_sets.get_mut(other_resource) {
                            conflict_set.insert(res_name.clone());
                        }
                        else {
                            conflict_sets.insert(other_resource.clone(), HashSet::from_iter([res_name.clone()]));
                        }
                    }

                    if let Some(mut conflict_set) = conflict_sets.get_mut(res_name) {
                        conflict_set.extend(conflicts.iter().map(|p| p.clone()));
                    }
                    else {
                        conflict_sets.insert(res_name.clone(), conflicts);
                    }
                }
            }
        }

        SparseScheduleConflictBranchAndBound {
            conflict_sets,
            fake_requests,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct ConstraintList {
    positive_constraints: HashMap<usize, usize>, 
    negative_constraints: HashSet<String>
}

impl Hash for ConstraintList {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        for p in &self.positive_constraints {
            p.hash(state);
        }

        for n in &self.positive_constraints {
            n.hash(state);
        }
    }
}

#[derive(Eq, PartialEq, Clone, Debug)]
struct Solution {
    assignments: HashMap<usize, usize>,
    cost: OrderedFloat<f64>,
    unallocated: usize,
    positive_constraints: HashMap<usize, usize>, 
    negative_constraints: HashSet<String>
}

impl Ord for Solution {

    // Flip comparison to make binary heap a min heap
    fn cmp(&self, other: &Self) -> Ordering {
        other.unallocated.cmp(&self.unallocated).then_with(|| other.cost.cmp(&self.cost))
    }
}

impl PartialOrd for Solution {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct SparseScheduleConflictBranchAndBound {
    conflict_sets: HashMap<String, HashSet<String>>,
    fake_requests: HashMap<usize, Vec<ReservationRequest>>,
}

impl SparseScheduleConflictBranchAndBound {
    fn debug_print(&self) {
        for (req_id, alts) in &self.fake_requests{
            for alt in alts {
                println!("{:?} {:?} {:?}  For {:?}", req_id, alt.parameters.resource_name, alt.parameters.start_time.earliest_start, alt.parameters.start_time.latest_start);
            }
        }
    }
    fn greedy_allocate(&self, 
        positive_constraints: HashMap<usize, usize>, 
        negative_constraints: HashSet<String>) -> Solution {
        
        let mut assignments = HashMap::new();
        let mut total_score = 0.0;
        let mut unallocated = 0usize;
        for (req_id, res) in &self.fake_requests {
            if let Some(alt) = positive_constraints.get(&req_id) {
                assignments.insert(*req_id, *alt);
                let req = &res[*alt];
                total_score += req.cost_function.cost(&req.parameters, 
                    &req.parameters.start_time.earliest_start.unwrap());
                continue;
            }

            let mut min_score = f64::INFINITY;
            let mut selected_alt = None;
            for alt in  0..res.len() {
                let req = &res[alt];
                let score = req.cost_function.cost(&req.parameters, 
                    &req.parameters.start_time.earliest_start.unwrap());
                let resource = res[alt].parameters.resource_name.clone();
                if negative_constraints.contains(&resource) {
                    continue;
                }

                if min_score > score {
                    min_score = score;
                    selected_alt = Some(alt);
                }
            }

            let Some(alt) = selected_alt else {
                unallocated += 1;
                continue;
            };
            total_score += min_score;

            assignments.insert(*req_id, alt);
        }

        Solution {
            assignments, 
            cost: OrderedFloat(total_score), 
            unallocated,
            positive_constraints,
            negative_constraints
        }
    }

    /// Get implied banned stuff
    fn get_implications(&self) -> UniqueMultiHashMap<(usize, usize), String> {
        // Contains a  bunch of banned allocations
        // <Contenting allocation, resources_to_ban>
        let mut banned_allocations = UniqueMultiHashMap::new();
        
        for (req_id, alt) in &self.fake_requests {
            for alt_id in 0..alt.len() {
                let fake_req = &self.fake_requests[req_id][alt_id];
                if let Some(conflicts) = self.conflict_sets.get(&fake_req.parameters.resource_name) {
                    for conflict in conflicts {
                        banned_allocations.insert((*req_id, alt_id), conflict.clone());
                    }
                }
                banned_allocations.insert((*req_id, alt_id), fake_req.parameters.resource_name.clone());
            }
        }
        banned_allocations
    }

    fn get_conflicts(&self, allocations: &HashMap<usize, usize>, implications: &UniqueMultiHashMap<(usize, usize), String>) -> HashSet<(usize, usize)>{
        let mut conflicts = HashSet::new();
        for (req_id1, alt_id1) in allocations {
            for (req_id2, alt_id2) in allocations {
                if req_id1 == req_id2 {
                    // No need to check alternatives as only one request will be satisfied
                    continue;
                }
                let Some(imp1) = implications.get(&(*req_id1, *alt_id1)) else {
                    continue;
                };

                let Some(imp2) = implications.get(&(*req_id1, *alt_id2)) else {
                    continue;
                };

                if imp1 == imp2 {
                    conflicts.insert((*req_id1, *alt_id1));
                    conflicts.insert((*req_id2, *alt_id2));
                }
            }
        }

        conflicts
    }

    /// Check conflict by remapping timelines
    pub fn solve(&self) -> Option<Solution> {
        let solution =
            self.greedy_allocate(HashMap::new(), HashSet::new());
        let implications = self.get_implications();
        let mut priority_queue = BinaryHeap::new();
        priority_queue.push(solution);

        while let Some(solution) = priority_queue.pop() {
            let conflicts = self.get_conflicts(&solution.assignments, &implications);
            println!("Got solution {:?}", solution);
            if conflicts.len() == 0 {
                return Some(solution.clone());
            }
            else {
                println!("Conflicts: {:?}", conflicts);
            }

            for (req_id, alt) in conflicts.iter() {
                let mut positive_constraints = solution.positive_constraints.clone();
                let mut negative_constraints = solution.negative_constraints.clone();
                positive_constraints.insert(*req_id, *alt);

                if let Some(banned_resources) = implications.get(&(*req_id, *alt)) {
                    negative_constraints.extend(banned_resources.iter().map(|p| p.clone()));
                }
                
                let solution = self.greedy_allocate(positive_constraints, negative_constraints);
                priority_queue.push(solution);
            }
        }
        None
    }
}


#[cfg(test)]
#[test]
fn test_conflict_checker() {
    use std::sync::Arc;
    use chrono::{TimeZone, Duration};

    use crate::{ReservationParameters, StartTimeRange, cost_function::static_cost::StaticCost};


    let resources = vec!["station1".to_string(), "station2".to_string()];
    let alternative1 = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: resources[0].clone(),
            start_time: StartTimeRange {
                earliest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap()),
                latest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 7, 10, 11).unwrap()),
            },
            duration: Some(Duration::minutes(30)),
        },
        cost_function: Arc::new(StaticCost::new(3.0)),
    };

    let alternative2 = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: resources[1].clone(),
            start_time: StartTimeRange {
                earliest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap()),
                latest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 7, 10, 11).unwrap()),
            },
            duration: Some(Duration::minutes(30)),
        },
        cost_function: Arc::new(StaticCost::new(6.0)),
    };


    let alternative1_cheaper = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: resources[0].clone(),
            start_time: StartTimeRange {
                earliest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap()),
                latest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 7, 10, 11).unwrap()),
            },
            duration: Some(Duration::minutes(30)),
        },
        cost_function: Arc::new(StaticCost::new(2.0)),
    };


    let req1 = vec![alternative1, alternative2];
    let req2 = vec![alternative1_cheaper];

    let mut system = TimeBasedBranchAndBound::create_with_resources(&resources);

    system.request_resources(req1);
    system.request_resources(req2);

    let solver = system.generate_literals_and_remap_requests();
    println!("Generated literals");
    solver.debug_print();
    let solution = solver.solve().unwrap();
    assert!((solution.cost.0 - 8.0).abs() < 1.0);
}


fn generate_test_scenario(num_resources: usize, max_requests_per_resource: usize) -> (Vec<Vec<ReservationRequest>>, HashSet<usize, usize>) {

    let resources = Vec::from_iter(
        (0..num_resources).map(|m| format!("Station {}", i)));
}